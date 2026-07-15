//! An [`object_store`]-backed [`RemoteStorage`] implementation for rollforward.
//!
//! rollforward's remote is "dumb": it stores and lists opaque objects and needs
//! exactly one atomic primitive — a create-if-absent append (`put_oplog_cas`) to
//! detect concurrent writers at the same sequence. `object_store`'s
//! [`PutMode::Create`] provides that on S3 (and on the local-filesystem backend
//! used by tests), returning [`ObjectStoreError::AlreadyExists`] on collision.
//!
//! The object key layout mirrors rollforward's own `LocalFolderRemote`:
//!
//! ```text
//! <prefix>/<file_id>/oplogs/{seq}_{client}.oplog
//! <prefix>/<file_id>/baselines/baseline_<seq>.zst
//! <prefix>/packs/<pack_id>
//! <prefix>/pack-indexes/<pack_id>
//! <prefix>/clients_status/<client>.status
//! <prefix>/_inventory/files/<base64url-file-id>
//! <prefix>/_inventory/snapshot-v1.json
//! <prefix>/_inventory/ready-v1
//! ```
//!
//! ## Async/sync bridge
//! `RemoteStorage`'s methods are synchronous but `object_store` is async. The
//! plugin runs on a multi-thread Tokio runtime, so each method bridges with
//! [`tokio::task::block_in_place`] over a stored runtime [`Handle`] — valid
//! because the engine calls these from a normal worker thread, never from
//! inside another `block_on`.

#[cfg(any())]
use base64::Engine as _;
use futures::StreamExt as _;
use object_store::path::Path as ObjPath;
use object_store::{
    Error as ObjectStoreError, ObjectStore, PutMode, PutOptions,
};
#[cfg(any())]
use rollforward::RemoteStorage;
#[cfg(any())]
use rollforward::types::{ClientStatus, OpLogEntry, RemoteLogItem};
use rollforward::{
    CatalogCompaction, CatalogCursor, CatalogDelta, CatalogEvent,
    CatalogScanRequest, CatalogSnapshot, ChunkLocation, Commit, CommitBatch,
    CommitBatchResult, PackRange, RangeData, ResourceAck, ResourceKey,
};
use rollforward::{RemoteStorageV2, SyncError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
/// The `.oplog` filename suffix, matching rollforward's `oplog` module.
#[cfg(any())]
const OPLOG_EXT: &str = "oplog";

/// A [`RemoteStorage`] backed by any [`ObjectStore`] (S3, local folder, …).
pub struct S3Remote {
    /// The underlying object store (e.g. `AmazonS3` or `LocalFileSystem`).
    store: Arc<dyn ObjectStore>,
    /// Key prefix under which all objects for this library live.
    prefix: String,
    /// Handle to the multi-thread runtime used to drive async store calls.
    handle: Handle,
    /// Bounds pack uploads independently from item-level concurrency.
    pack_upload_slots: Arc<Semaphore>,
    /// Completed upload measurements waiting to be forwarded to daemon logs.
    upload_stats: Mutex<Vec<PackUploadStat>>,
    v2_pack_index_ids: Mutex<Option<Vec<String>>>,
    v2_pack_index_cache: Mutex<HashMap<String, Vec<u8>>>,
}

/// Measurement for one pack plus its small index object.
#[derive(Debug, Clone)]
pub struct PackUploadStat {
    pub pack_id: String,
    pub bytes: usize,
    pub elapsed: Duration,
    pub queue_wait: Duration,
    pub pack_elapsed: Duration,
    pub index_elapsed: Duration,
}

impl PackUploadStat {
    /// Compact human-readable line suitable for daemon logs.
    pub fn log_message(&self) -> String {
        let seconds = self.elapsed.as_secs_f64().max(0.001);
        let mib = self.bytes as f64 / (1024.0 * 1024.0);
        let rate = mib / seconds;
        let short_id = &self.pack_id[..self.pack_id.len().min(12)];
        format!(
            "pack {short_id} uploaded: {mib:.2} MiB in {seconds:.2}s ({rate:.2} MiB/s), queue={:.2}s pack_put={:.2}s index_put={:.2}s",
            self.queue_wait.as_secs_f64(),
            self.pack_elapsed.as_secs_f64(),
            self.index_elapsed.as_secs_f64(),
        )
    }
}

impl std::fmt::Debug for S3Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Remote")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

/// Map any `object_store` error into a rollforward [`SyncError::IoError`].
fn io(e: impl std::fmt::Display) -> SyncError {
    SyncError::IoError { msg: e.to_string() }
}

/// `{seq}_{client}.oplog` — the oplog object filename for a sequence/client.
#[cfg(any())]
fn oplog_name(sequence: u64, client_id: &str) -> String {
    format!("{sequence}_{client_id}.{OPLOG_EXT}")
}

/// Parse `{seq}_{client}.oplog` back into `(sequence, client_id)`.
#[cfg(any())]
fn parse_oplog_name(name: &str) -> Option<(u64, String)> {
    let stem = name.strip_suffix(&format!(".{OPLOG_EXT}"))?;
    let (seq, client) = stem.split_once('_')?;
    Some((seq.parse().ok()?, client.to_owned()))
}

impl S3Remote {
    /// Build a remote over `store`, rooting every key under `prefix`.
    #[cfg(test)]
    pub fn new(
        store: Arc<dyn ObjectStore>,
        prefix: impl Into<String>,
        handle: Handle,
    ) -> Self {
        Self::new_with_concurrency(store, prefix, handle, 4)
    }

    /// Build a remote with an explicit maximum number of in-flight pack PUTs.
    pub fn new_with_concurrency(
        store: Arc<dyn ObjectStore>,
        prefix: impl Into<String>,
        handle: Handle,
        pack_upload_concurrency: usize,
    ) -> Self {
        let mut prefix = prefix.into();
        while prefix.ends_with('/') {
            prefix.pop();
        }
        Self {
            store,
            prefix,
            handle,
            pack_upload_slots: Arc::new(Semaphore::new(
                pack_upload_concurrency.max(1),
            )),
            upload_stats: Mutex::new(Vec::new()),
            v2_pack_index_ids: Mutex::new(None),
            v2_pack_index_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Return completed pack measurements, removing them from the remote.
    pub fn take_upload_stats(&self) -> Vec<PackUploadStat> {
        std::mem::take(
            &mut *self
                .upload_stats
                .lock()
                .expect("upload stats lock poisoned"),
        )
    }

    /// Persist one complete inventory snapshot. New IDs are already made
    /// durable as immutable markers before their first oplog is published.
    #[cfg(any())]
    #[allow(dead_code)] // The integration test includes this module without the plugin runner.
    pub fn persist_inventory(&self) -> Result<usize, SyncError> {
        let mut ids: Vec<String> = self
            .known_inventory_ids
            .lock()
            .expect("inventory ids lock poisoned")
            .iter()
            .cloned()
            .collect();
        ids.sort();
        let bytes = serde_json::to_vec(&ids).map_err(|error| {
            SyncError::SerdeError { msg: error.to_string() }
        })?;
        let snapshot_key = self.key(&["_inventory", "snapshot-v1.json"]);
        self.block(async {
            self.store.put(&snapshot_key, bytes.into()).await
        })
        .map_err(io)?;
        Ok(ids.len())
    }

    /// Durably register file IDs before publishing their first oplogs. The
    /// immutable markers are uploaded concurrently, closing the crash window
    /// without adding one serial network round trip to every small file.
    #[cfg(any())]
    pub fn register_inventory_ids(
        &self,
        file_ids: &[String],
    ) -> Result<usize, SyncError> {
        let known = self
            .known_inventory_ids
            .lock()
            .expect("inventory ids lock poisoned");
        let mut pending: Vec<String> = file_ids
            .iter()
            .filter(|file_id| !known.contains(file_id.as_str()))
            .cloned()
            .collect();
        drop(known);
        pending.sort();
        pending.dedup();
        if pending.is_empty() {
            return Ok(0);
        }

        let store = self.store.clone();
        let marker_keys: Vec<ObjPath> = pending
            .iter()
            .map(|file_id| {
                self.key(&[
                    "_inventory",
                    "files",
                    &Self::inventory_name(file_id),
                ])
            })
            .collect();
        self.block(async move {
            let mut uploads =
                futures::stream::iter(marker_keys.into_iter().map(|key| {
                    let store = store.clone();
                    async move { store.put(&key, Vec::new().into()).await }
                }))
                .buffer_unordered(32);
            while let Some(result) = uploads.next().await {
                result?;
            }
            Ok::<(), ObjectStoreError>(())
        })
        .map_err(io)?;
        self.known_inventory_ids
            .lock()
            .expect("inventory ids lock poisoned")
            .extend(pending.iter().cloned());
        Ok(pending.len())
    }

    /// Join `segments` under the configured prefix into an object key.
    fn key(&self, segments: &[&str]) -> ObjPath {
        let mut parts: Vec<&str> = Vec::with_capacity(segments.len() + 1);
        if !self.prefix.is_empty() {
            parts.push(self.prefix.as_str());
        }
        parts.extend_from_slice(segments);
        ObjPath::from(parts.join("/"))
    }

    fn v2_key(&self, segments: &[&str]) -> ObjPath {
        let mut all = vec!["_rollforward", "v2"];
        all.extend_from_slice(segments);
        self.key(&all)
    }

    fn object_name(value: &str) -> String {
        blake3::hash(value.as_bytes()).to_hex().to_string()
    }

    fn list_paths(&self, prefix: &ObjPath) -> Result<Vec<ObjPath>, SyncError> {
        self.block(async {
            let mut stream = self.store.list(Some(prefix));
            let mut paths = Vec::new();
            while let Some(meta) = stream.next().await {
                paths.push(meta.map_err(io)?.location);
            }
            Ok(paths)
        })
    }

    fn put_create(
        &self,
        key: &ObjPath,
        bytes: Vec<u8>,
    ) -> Result<(), SyncError> {
        let options =
            PutOptions { mode: PutMode::Create, ..Default::default() };
        match self.block(async {
            self.store.put_opts(key, bytes.into(), options).await
        }) {
            Ok(_) | Err(ObjectStoreError::AlreadyExists { .. }) => Ok(()),
            Err(error) => Err(io(error)),
        }
    }

    /// Run an async store operation to completion on the stored runtime.
    fn block<F: Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.handle.block_on(fut))
    }

    /// Spawn one pack/index pair. They upload concurrently to pay one network
    /// RTT; oplog publication remains the commit barrier. If the pack fails
    /// after the index succeeds, remove the unsafe orphan index best-effort.
    #[cfg(any())]
    fn spawn_pack_upload(
        &self,
        pack_id: String,
        pack_bytes: Vec<u8>,
        index_bytes: Option<Vec<u8>>,
    ) -> JoinHandle<Result<PackUploadStat, String>> {
        let store = self.store.clone();
        let slots = self.pack_upload_slots.clone();
        let pack_key = self.key(&["packs", &pack_id]);
        let index_key = self.key(&["pack-indexes", &pack_id]);
        self.handle.spawn(async move {
            let queued_at = Instant::now();
            let _permit = slots
                .acquire_owned()
                .await
                .map_err(|_| "pack upload queue closed".to_owned())?;
            let queue_wait = queued_at.elapsed();
            let started = Instant::now();
            let bytes = pack_bytes.len();
            let pack_started = Instant::now();
            let pack_upload = async {
                let result = store.put(&pack_key, pack_bytes.into()).await;
                (result, pack_started.elapsed())
            };
            let index_started = Instant::now();
            let index_upload = async {
                let result = if let Some(index_bytes) = index_bytes {
                    store.put(&index_key, index_bytes.into()).await.map(|_| ())
                } else {
                    Ok(())
                };
                (result, index_started.elapsed())
            };
            let ((pack_result, pack_elapsed), (index_result, index_elapsed)) =
                tokio::join!(pack_upload, index_upload);
            if let Err(error) = pack_result {
                if index_result.is_ok() {
                    let _ = store.delete(&index_key).await;
                }
                return Err(format!("upload pack {pack_id}: {error}"));
            }
            index_result.map_err(|error| {
                format!("upload pack index {pack_id}: {error}")
            })?;
            Ok(PackUploadStat {
                pack_id,
                bytes,
                elapsed: started.elapsed(),
                queue_wait,
                pack_elapsed,
                index_elapsed,
            })
        })
    }

    /// Wait for every pack queued before this call and surface background
    /// errors synchronously. Oplog publication calls this first, making it the
    /// commit barrier for a binary manifest.
    #[cfg(any())]
    fn flush_pack_uploads(&self) -> Result<(), SyncError> {
        let _flush = self.flush_lock.lock().expect("pack flush lock poisoned");
        let handles = {
            let mut state =
                self.upload_state.lock().expect("upload state lock poisoned");
            let pending = std::mem::take(&mut state.pending_packs);
            for (pack_id, bytes) in pending {
                let upload = self.spawn_pack_upload(pack_id, bytes, None);
                state.uploads.push(upload);
            }
            std::mem::take(&mut state.uploads)
        };
        if handles.is_empty() {
            return Ok(());
        }

        let results =
            self.block(async { futures::future::join_all(handles).await });
        let mut first_error = None;
        let mut completed =
            self.upload_stats.lock().expect("upload stats lock poisoned");
        for result in results {
            match result {
                Ok(Ok(stat)) => completed.push(stat),
                Ok(Err(error)) => {
                    first_error.get_or_insert(error);
                }
                Err(error) => {
                    first_error.get_or_insert_with(|| {
                        format!("pack upload worker failed: {error}")
                    });
                }
            }
        }
        match first_error {
            Some(error) => Err(io(error)),
            None => Ok(()),
        }
    }

    /// Fetch an object's bytes, or `None` if it does not exist.
    fn get_opt(&self, key: &ObjPath) -> Result<Option<Vec<u8>>, SyncError> {
        self.block(async {
            match self.store.get(key).await {
                Ok(res) => Ok(Some(res.bytes().await.map_err(io)?.to_vec())),
                Err(ObjectStoreError::NotFound { .. }) => Ok(None),
                Err(e) => Err(io(e)),
            }
        })
    }

    /// List the leaf names of objects directly under `dir` (one level deep).
    #[cfg(any())]
    fn list_names(&self, dir: &ObjPath) -> Result<Vec<String>, SyncError> {
        self.block(async {
            let mut stream = self.store.list(Some(dir));
            let mut names = Vec::new();
            while let Some(meta) = stream.next().await {
                let meta = meta.map_err(io)?;
                if let Some(name) = meta.location.filename() {
                    names.push(name.to_owned());
                }
            }
            Ok(names)
        })
    }

    #[cfg(any())]
    fn inventory_name(file_id: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(file_id)
    }

    #[cfg(any())]
    fn inventory_file_id(name: &str) -> Option<String> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(name)
            .ok()?;
        String::from_utf8(bytes).ok()
    }

    #[cfg(any())]
    fn put_inventory_marker(&self, file_id: &str) -> Result<(), SyncError> {
        let known = self
            .known_inventory_ids
            .lock()
            .expect("inventory ids lock poisoned")
            .contains(file_id);
        if known {
            return Ok(());
        }
        self.register_inventory_ids(&[file_id.to_owned()]).map(|_| ())
    }

    #[cfg(any())]
    fn list_inventory_file_ids(&self) -> Result<Vec<String>, SyncError> {
        let names = self.list_names(&self.key(&["_inventory", "files"]))?;
        let mut ids: Vec<String> = names
            .iter()
            .filter_map(|name| Self::inventory_file_id(name))
            .collect();
        if let Some(snapshot) =
            self.get_opt(&self.key(&["_inventory", "snapshot-v1.json"]))?
        {
            let snapshot_ids: Vec<String> = serde_json::from_slice(&snapshot)
                .map_err(|error| SyncError::SerdeError {
                    msg: format!("invalid inventory snapshot: {error}"),
                })?;
            ids.extend(snapshot_ids);
        }
        ids.extend(
            self.known_inventory_ids
                .lock()
                .expect("inventory ids lock poisoned")
                .iter()
                .cloned(),
        );
        ids.sort();
        ids.dedup();
        self.known_inventory_ids
            .lock()
            .expect("inventory ids lock poisoned")
            .extend(ids.iter().cloned());
        Ok(ids)
    }

    #[cfg(any())]
    fn legacy_list_file_ids(&self) -> Result<Vec<String>, SyncError> {
        let base = self.key(&[]);
        let strip = if self.prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", self.prefix)
        };
        self.block(async {
            let mut stream = self.store.list(Some(&base));
            let mut ids = std::collections::BTreeSet::new();
            while let Some(meta) = stream.next().await {
                let meta = meta.map_err(io)?;
                let key = meta.location.as_ref();
                let rel = key.strip_prefix(&strip).unwrap_or(key);
                if let Some((file_id, _object)) = rel.rsplit_once("/oplogs/")
                    && !file_id.is_empty()
                {
                    ids.insert(file_id.to_owned());
                }
            }
            Ok(ids.into_iter().collect())
        })
    }

    /// Distinct file ids that have oplog history under the prefix. File ids may
    /// themselves contain slashes, so `/oplogs/` is the structural boundary.
    /// reserved global stores (`packs`, `pack-indexes`, `clients_status`) never
    /// have an `oplogs` child, so they are excluded by construction.
    #[cfg(any())]
    fn list_file_ids(&self) -> Result<Vec<String>, SyncError> {
        let ready = self.key(&["_inventory", "ready-v1"]);
        if self.get_opt(&ready)?.is_some() {
            return self.list_inventory_file_ids();
        }

        // One-time migration for existing buckets. Only this first run scans
        // the legacy mixed prefix; later runs list the compact marker prefix.
        let ids = self.legacy_list_file_ids()?;
        let snapshot = serde_json::to_vec(&ids).map_err(|error| {
            SyncError::SerdeError { msg: error.to_string() }
        })?;
        let snapshot_key = self.key(&["_inventory", "snapshot-v1.json"]);
        self.block(async {
            self.store.put(&snapshot_key, snapshot.into()).await
        })
        .map_err(io)?;
        self.block(async {
            self.store.put(&ready, b"ready-v1".to_vec().into()).await
        })
        .map_err(io)?;
        self.known_inventory_ids
            .lock()
            .expect("inventory ids lock poisoned")
            .extend(ids.iter().cloned());
        Ok(ids)
    }
}

#[cfg(any())]
impl RemoteStorage for S3Remote {
    fn list_files(&self) -> Result<Vec<String>, SyncError> {
        self.list_file_ids()
    }

    fn list_oplogs(
        &self,
        file_id: String,
    ) -> Result<Vec<RemoteLogItem>, SyncError> {
        let dir = self.key(&[&file_id, "oplogs"]);
        let mut out = Vec::new();
        for name in self.list_names(&dir)? {
            if let Some((sequence, client_id)) = parse_oplog_name(&name) {
                out.push(RemoteLogItem {
                    sequence,
                    client_id,
                    remote_path: name,
                });
            }
        }
        Ok(out)
    }

    fn put_oplog(
        &self,
        file_id: String,
        entry: OpLogEntry,
    ) -> Result<(), SyncError> {
        self.flush_pack_uploads()?;
        self.put_inventory_marker(&file_id)?;
        let key = self.key(&[
            &file_id,
            "oplogs",
            &oplog_name(entry.sequence, &entry.client_id),
        ]);
        let bytes = serde_json::to_vec(&entry)
            .map_err(|e| SyncError::SerdeError { msg: e.to_string() })?;
        self.block(async { self.store.put(&key, bytes.into()).await })
            .map_err(io)?;
        Ok(())
    }

    fn put_oplog_cas(
        &self,
        file_id: String,
        entry: OpLogEntry,
    ) -> Result<(), SyncError> {
        self.flush_pack_uploads()?;
        self.put_inventory_marker(&file_id)?;
        // Match rollforward's `LocalFolderRemote`: a dumb remote can't offer a
        // true atomic CAS across clients, so reject the append if *any* client
        // already claimed this sequence (oplog names embed the client id, so
        // create-if-absent on our own key alone wouldn't catch a peer). The
        // `PutMode::Create` below still guards the exact same-key race.
        let taken = self
            .list_oplogs(file_id.clone())?
            .iter()
            .any(|i| i.sequence == entry.sequence);
        if taken {
            return Err(SyncError::Conflict { sequence: entry.sequence });
        }
        let key = self.key(&[
            &file_id,
            "oplogs",
            &oplog_name(entry.sequence, &entry.client_id),
        ]);
        let bytes = serde_json::to_vec(&entry)
            .map_err(|e| SyncError::SerdeError { msg: e.to_string() })?;
        let opts = PutOptions { mode: PutMode::Create, ..Default::default() };
        match self.block(async {
            self.store.put_opts(&key, bytes.into(), opts).await
        }) {
            Ok(_) => Ok(()),
            Err(ObjectStoreError::AlreadyExists { .. }) => {
                Err(SyncError::Conflict { sequence: entry.sequence })
            }
            Err(e) => Err(io(e)),
        }
    }

    fn get_oplog(
        &self,
        file_id: String,
        remote_path: String,
    ) -> Result<Vec<u8>, SyncError> {
        let key = self.key(&[&file_id, "oplogs", &remote_path]);
        self.get_opt(&key)?
            .ok_or_else(|| io(format!("oplog not found: {remote_path}")))
    }

    fn delete_oplog(
        &self,
        file_id: String,
        remote_path: String,
    ) -> Result<(), SyncError> {
        let key = self.key(&[&file_id, "oplogs", &remote_path]);
        match self.block(async { self.store.delete(&key).await }) {
            Ok(()) | Err(ObjectStoreError::NotFound { .. }) => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    fn put_pack(
        &self,
        pack_id: String,
        data: Vec<u8>,
    ) -> Result<(), SyncError> {
        self.upload_state
            .lock()
            .expect("upload state lock poisoned")
            .pending_packs
            .insert(pack_id, data);
        Ok(())
    }

    fn get_pack_range(
        &self,
        pack_id: String,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, SyncError> {
        self.flush_pack_uploads()?;
        let key = self.key(&["packs", &pack_id]);
        let start = usize::try_from(offset).map_err(io)?;
        let end = start
            .checked_add(length as usize)
            .ok_or_else(|| io("pack range overflow"))?;
        let bytes = self
            .block(async { self.store.get_range(&key, start..end).await })
            .map_err(io)?;
        Ok(bytes.to_vec())
    }

    fn list_packs(&self) -> Result<Vec<String>, SyncError> {
        self.flush_pack_uploads()?;
        self.list_names(&self.key(&["packs"]))
    }

    fn delete_pack(&self, pack_id: String) -> Result<(), SyncError> {
        self.flush_pack_uploads()?;
        let key = self.key(&["packs", &pack_id]);
        match self.block(async { self.store.delete(&key).await }) {
            Ok(()) | Err(ObjectStoreError::NotFound { .. }) => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    fn put_pack_index(
        &self,
        index_id: String,
        data: Vec<u8>,
    ) -> Result<(), SyncError> {
        {
            self.pack_index_cache
                .lock()
                .expect("pack index cache lock poisoned")
                .insert(index_id.clone(), data.clone());
            if let Some(ids) = self
                .pack_index_ids
                .lock()
                .expect("pack index ids lock poisoned")
                .as_mut()
                && !ids.contains(&index_id)
            {
                ids.push(index_id.clone());
                ids.sort();
            }
        }
        let mut state =
            self.upload_state.lock().expect("upload state lock poisoned");
        if let Some(pack_bytes) = state.pending_packs.remove(&index_id) {
            let upload =
                self.spawn_pack_upload(index_id, pack_bytes, Some(data));
            state.uploads.push(upload);
            return Ok(());
        }
        drop(state);

        // Maintenance code may write an index independently. Ensure a matching
        // content-addressed pack already in flight has completed first.
        self.flush_pack_uploads()?;
        let key = self.key(&["pack-indexes", &index_id]);
        self.block(async { self.store.put(&key, data.into()).await })
            .map_err(io)?;
        Ok(())
    }

    fn get_pack_index(&self, index_id: String) -> Result<Vec<u8>, SyncError> {
        if let Some(bytes) = self
            .pack_index_cache
            .lock()
            .expect("pack index cache lock poisoned")
            .get(&index_id)
            .cloned()
        {
            return Ok(bytes);
        }
        self.flush_pack_uploads()?;
        let key = self.key(&["pack-indexes", &index_id]);
        let bytes = self
            .get_opt(&key)?
            .ok_or_else(|| io(format!("pack index not found: {index_id}")))?;
        self.pack_index_cache
            .lock()
            .expect("pack index cache lock poisoned")
            .insert(index_id, bytes.clone());
        Ok(bytes)
    }

    fn list_pack_indexes(&self) -> Result<Vec<String>, SyncError> {
        if let Some(ids) = self
            .pack_index_ids
            .lock()
            .expect("pack index ids lock poisoned")
            .clone()
        {
            return Ok(ids);
        }
        self.flush_pack_uploads()?;
        let mut ids = self.list_names(&self.key(&["pack-indexes"]))?;
        ids.sort();
        *self.pack_index_ids.lock().expect("pack index ids lock poisoned") =
            Some(ids.clone());
        Ok(ids)
    }

    fn delete_pack_index(&self, index_id: String) -> Result<(), SyncError> {
        self.flush_pack_uploads()?;
        self.pack_index_cache
            .lock()
            .expect("pack index cache lock poisoned")
            .remove(&index_id);
        if let Some(ids) = self
            .pack_index_ids
            .lock()
            .expect("pack index ids lock poisoned")
            .as_mut()
        {
            ids.retain(|id| id != &index_id);
        }
        let key = self.key(&["pack-indexes", &index_id]);
        match self.block(async { self.store.delete(&key).await }) {
            Ok(()) | Err(ObjectStoreError::NotFound { .. }) => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    fn put_baseline(
        &self,
        file_id: String,
        seq: u64,
        data: Vec<u8>,
    ) -> Result<(), SyncError> {
        let key = self.key(&[&file_id, "baselines", &baseline_name(seq)]);
        self.block(async { self.store.put(&key, data.into()).await })
            .map_err(io)?;
        Ok(())
    }

    fn get_baseline(
        &self,
        file_id: String,
        seq: u64,
    ) -> Result<Option<Vec<u8>>, SyncError> {
        let key = self.key(&[&file_id, "baselines", &baseline_name(seq)]);
        self.get_opt(&key)
    }

    fn list_baselines(&self, file_id: String) -> Result<Vec<u64>, SyncError> {
        let dir = self.key(&[&file_id, "baselines"]);
        let mut out = Vec::new();
        for name in self.list_names(&dir)? {
            if let Some(seq) = name
                .strip_prefix("baseline_")
                .and_then(|s| s.strip_suffix(".zst"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                out.push(seq);
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    fn put_status(
        &self,
        client_id: String,
        last_synced_sequence: u64,
    ) -> Result<(), SyncError> {
        let key =
            self.key(&["clients_status", &format!("{client_id}.status")]);
        let body = serde_json::json!({ "last_synced_sequence": last_synced_sequence });
        let bytes = serde_json::to_vec(&body)
            .map_err(|e| SyncError::SerdeError { msg: e.to_string() })?;
        self.block(async { self.store.put(&key, bytes.into()).await })
            .map_err(io)?;
        Ok(())
    }

    fn list_statuses(&self) -> Result<Vec<ClientStatus>, SyncError> {
        let dir = self.key(&["clients_status"]);
        let mut out = Vec::new();
        for name in self.list_names(&dir)? {
            let Some(client) = name.strip_suffix(".status") else { continue };
            let key = self.key(&["clients_status", &name]);
            let Some(bytes) = self.get_opt(&key)? else { continue };
            let val: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| SyncError::SerdeError { msg: e.to_string() })?;
            let seq = val
                .get("last_synced_sequence")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| SyncError::SerdeError {
                    msg: format!("bad status file: {name}"),
                })?;
            out.push(ClientStatus {
                client_id: client.to_owned(),
                last_synced_sequence: seq,
            });
        }
        Ok(out)
    }
}

/// `baseline_<seq>.zst`
#[cfg(any())]
fn baseline_name(seq: u64) -> String {
    format!("baseline_{seq}.zst")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CatalogSegmentV2 {
    client_id: String,
    scope_id: String,
    first_counter: u64,
    last_counter: u64,
    events: Vec<CatalogEvent>,
}

impl RemoteStorageV2 for S3Remote {
    fn scan_catalog(
        &self,
        request: CatalogScanRequest,
    ) -> Result<CatalogDelta, SyncError> {
        let mut scopes = request.scopes;
        if scopes.is_empty() {
            for path in
                self.list_paths(&self.v2_key(&["catalog", "scopes"]))?
            {
                if let Some(bytes) = self.get_opt(&path)? {
                    scopes.push(String::from_utf8(bytes).map_err(io)?);
                }
            }
        }
        scopes.sort();
        scopes.dedup();
        let scope_hashes: HashMap<_, _> = scopes
            .iter()
            .map(|scope| (Self::object_name(scope), scope.clone()))
            .collect();

        let mut clients = Vec::new();
        for path in self.list_paths(&self.v2_key(&["catalog", "registry"]))? {
            if let Some(bytes) = self.get_opt(&path)? {
                clients.push(String::from_utf8(bytes).map_err(io)?);
            }
        }
        clients.sort();
        clients.dedup();

        let mut cursors: BTreeMap<(String, String), u64> = request
            .cursors
            .into_iter()
            .map(|cursor| {
                ((cursor.client_id, cursor.scope_id), cursor.counter)
            })
            .collect();
        let initial = cursors.clone();
        let mut covered = BTreeMap::<(String, String), u64>::new();
        let mut snapshot_cursors = Vec::new();
        let mut events = Vec::new();
        for scope in &scopes {
            let snapshot_dir = Self::object_name(scope);
            let latest_key = self.v2_key(&[
                "catalog",
                "snapshots",
                &snapshot_dir,
                "latest.json",
            ]);
            let Some(latest) = self.get_opt(&latest_key)? else { continue };
            let generation: u64 =
                serde_json::from_slice(&latest).map_err(|error| {
                    SyncError::SerdeError { msg: error.to_string() }
                })?;
            let snapshot_key = self.v2_key(&[
                "catalog",
                "snapshots",
                &snapshot_dir,
                &format!("{generation:020}.json"),
            ]);
            let bytes = self.get_opt(&snapshot_key)?.ok_or_else(|| {
                io("catalog snapshot manifest points to a missing generation")
            })?;
            let snapshot: CatalogSnapshot = serde_json::from_slice(&bytes)
                .map_err(|error| SyncError::SerdeError {
                    msg: error.to_string(),
                })?;
            for cursor in snapshot.cursors {
                snapshot_cursors.push(cursor.clone());
                let key = (cursor.client_id, cursor.scope_id);
                covered.insert(key.clone(), cursor.counter);
                cursors
                    .entry(key)
                    .and_modify(|value| *value = (*value).max(cursor.counter))
                    .or_insert(cursor.counter);
            }
            events.extend(snapshot.events.into_iter().filter(|event| {
                event.counter
                    > initial
                        .get(&(
                            event.client_id.clone(),
                            event.resource.scope_id.clone(),
                        ))
                        .copied()
                        .unwrap_or(0)
            }));
        }
        let mut candidates = Vec::new();
        for client in clients {
            let client_hash = Self::object_name(&client);
            let paths = if scopes.len() <= 32 {
                let mut paths = Vec::new();
                for scope in &scopes {
                    paths.extend(self.list_paths(&self.v2_key(&[
                        "catalog",
                        "clients",
                        &client_hash,
                        &Self::object_name(scope),
                    ]))?);
                }
                paths
            } else {
                self.list_paths(&self.v2_key(&[
                    "catalog",
                    "clients",
                    &client_hash,
                ]))?
            };
            for path in paths {
                let Some(filename) = path.filename() else { continue };
                let Some(last) = filename
                    .split('-')
                    .nth(1)
                    .and_then(|value| value.parse::<u64>().ok())
                else {
                    continue;
                };
                let path_text = path.as_ref();
                let Some(scope) = scope_hashes
                    .iter()
                    .find_map(|(hash, scope)| {
                        path_text
                            .contains(&format!("/{hash}/"))
                            .then_some(scope)
                    })
                    .cloned()
                else {
                    continue;
                };
                if last
                    > initial
                        .get(&(client.clone(), scope.clone()))
                        .copied()
                        .unwrap_or(0)
                        .max(
                            covered
                                .get(&(client.clone(), scope.clone()))
                                .copied()
                                .unwrap_or(0),
                        )
                {
                    candidates.push((client.clone(), scope, last, path));
                }
            }
        }

        let store = self.store.clone();
        let loaded = self
            .block(async move {
                let mut stream =
                    futures::stream::iter(candidates.into_iter().map(
                        |(client, scope, last, path)| {
                            let store = store.clone();
                            async move {
                                let bytes =
                                    store.get(&path).await?.bytes().await?;
                                Ok::<_, ObjectStoreError>((
                                    client,
                                    scope,
                                    last,
                                    bytes.to_vec(),
                                ))
                            }
                        },
                    ))
                    .buffer_unordered(32);
                let mut output = Vec::new();
                while let Some(result) = stream.next().await {
                    output.push(result?);
                }
                Ok::<_, ObjectStoreError>(output)
            })
            .map_err(io)?;

        for (client, scope, last, bytes) in loaded {
            let segment: CatalogSegmentV2 = serde_json::from_slice(&bytes)
                .map_err(|error| SyncError::SerdeError {
                    msg: error.to_string(),
                })?;
            let cursor = initial
                .get(&(client.clone(), scope.clone()))
                .copied()
                .unwrap_or(0)
                .max(
                    covered
                        .get(&(client.clone(), scope.clone()))
                        .copied()
                        .unwrap_or(0),
                );
            events.extend(
                segment
                    .events
                    .into_iter()
                    .filter(|event| event.counter > cursor),
            );
            cursors
                .entry((client, scope))
                .and_modify(|counter| *counter = (*counter).max(last))
                .or_insert(last);
        }
        events.sort_by(|left, right| {
            (&left.client_id, left.counter)
                .cmp(&(&right.client_id, right.counter))
        });
        Ok(CatalogDelta {
            events,
            cursors: cursors
                .into_iter()
                .map(|((client_id, scope_id), counter)| CatalogCursor {
                    client_id,
                    scope_id,
                    counter,
                })
                .collect(),
            snapshot_cursors,
        })
    }

    fn load_commits(
        &self,
        ids: Vec<String>,
    ) -> Result<Vec<Commit>, SyncError> {
        let store = self.store.clone();
        let keys: Vec<_> = ids
            .iter()
            .map(|id| self.v2_key(&["commits", &Self::object_name(id)]))
            .collect();
        let bytes = self
            .block(async move {
                let mut stream =
                    futures::stream::iter(keys.into_iter().map(|key| {
                        let store = store.clone();
                        async move { store.get(&key).await?.bytes().await }
                    }))
                    .buffer_unordered(32);
                let mut output = Vec::new();
                while let Some(result) = stream.next().await {
                    output.push(result?.to_vec());
                }
                Ok::<_, ObjectStoreError>(output)
            })
            .map_err(io)?;
        bytes
            .into_iter()
            .map(|bytes| {
                serde_json::from_slice(&bytes).map_err(|error| {
                    SyncError::SerdeError { msg: error.to_string() }
                })
            })
            .collect()
    }

    fn commit_batch(
        &self,
        batch: CommitBatch,
    ) -> Result<CommitBatchResult, SyncError> {
        let indexes: HashMap<_, _> = batch
            .indexes
            .into_iter()
            .map(|index| (index.id, index.data))
            .collect();
        let store = self.store.clone();
        let slots = self.pack_upload_slots.clone();
        let paired_ids: HashSet<_> =
            batch.packs.iter().map(|pack| pack.id.clone()).collect();
        let pack_jobs: Vec<_> = batch
            .packs
            .into_iter()
            .map(|pack| {
                let pack_key = self.v2_key(&["packs", &pack.id]);
                let index_key = self.v2_key(&["pack-indexes", &pack.id]);
                let index = indexes.get(&pack.id).cloned();
                (pack, pack_key, index_key, index)
            })
            .collect();
        let stats = self
            .block(async move {
                let mut uploads =
                    futures::stream::iter(pack_jobs.into_iter().map(
                        |(pack, pack_key, index_key, index)| {
                            let store = store.clone();
                            let slots = slots.clone();
                            async move {
                                let queued = Instant::now();
                                let _permit =
                                    slots.acquire_owned().await.map_err(
                                        |error| ObjectStoreError::Generic {
                                            store: "s3sync",
                                            source: Box::new(error),
                                        },
                                    )?;
                                let queue_wait = queued.elapsed();
                                let started = Instant::now();
                                let pack_put = async {
                                    let started = Instant::now();
                                    let result = store
                                        .put(
                                            &pack_key,
                                            pack.data.clone().into(),
                                        )
                                        .await;
                                    (result, started.elapsed())
                                };
                                let index_put = async {
                                    let started = Instant::now();
                                    if let Some(index) = index {
                                        let result = store
                                            .put(&index_key, index.into())
                                            .await
                                            .map(|_| ());
                                        (result, started.elapsed())
                                    } else {
                                        (Ok(()), started.elapsed())
                                    }
                                };
                                let (
                                    (pack_result, pack_elapsed),
                                    (index_result, index_elapsed),
                                ) = tokio::join!(pack_put, index_put);
                                pack_result?;
                                index_result?;
                                Ok::<_, ObjectStoreError>(PackUploadStat {
                                    pack_id: pack.id,
                                    bytes: pack.data.len(),
                                    elapsed: started.elapsed(),
                                    queue_wait,
                                    pack_elapsed,
                                    index_elapsed,
                                })
                            }
                        },
                    ))
                    .buffer_unordered(32);
                let mut output = Vec::new();
                while let Some(result) = uploads.next().await {
                    output.push(result?);
                }
                Ok::<_, ObjectStoreError>(output)
            })
            .map_err(io)?;
        self.upload_stats
            .lock()
            .expect("upload stats lock poisoned")
            .extend(stats);

        for (id, data) in indexes {
            self.v2_pack_index_cache
                .lock()
                .expect("v2 pack index cache lock poisoned")
                .insert(id.clone(), data.clone());
            if let Some(ids) = self
                .v2_pack_index_ids
                .lock()
                .expect("v2 pack index ids lock poisoned")
                .as_mut()
                && !ids.contains(&id)
            {
                ids.push(id.clone());
                ids.sort();
            }
            if !paired_ids.contains(&id) {
                let _ = self
                    .put_create(&self.v2_key(&["pack-indexes", &id]), data);
            }
        }

        for commit in &batch.commits {
            self.put_create(
                &self.v2_key(&["commits", &Self::object_name(&commit.id)]),
                serde_json::to_vec(commit).map_err(|error| {
                    SyncError::SerdeError { msg: error.to_string() }
                })?,
            )?;
        }

        let mut groups: BTreeMap<(String, String), Vec<&Commit>> =
            BTreeMap::new();
        for commit in &batch.commits {
            groups
                .entry((
                    commit.author.clone(),
                    commit.resource.scope_id.clone(),
                ))
                .or_default()
                .push(commit);
        }
        for ((client, scope), mut commits) in groups {
            commits.sort_by_key(|commit| commit.client_counter);
            let events: Vec<_> = commits
                .iter()
                .map(|commit| CatalogEvent {
                    client_id: client.clone(),
                    counter: commit.client_counter,
                    commit_id: commit.id.clone(),
                    resource: commit.resource.clone(),
                })
                .collect();
            let first_counter =
                events.first().map_or(0, |event| event.counter);
            let last_counter = events.last().map_or(0, |event| event.counter);
            let segment = CatalogSegmentV2 {
                client_id: client.clone(),
                scope_id: scope.clone(),
                first_counter,
                last_counter,
                events,
            };
            let bytes = serde_json::to_vec(&segment).map_err(|error| {
                SyncError::SerdeError { msg: error.to_string() }
            })?;
            let digest = blake3::hash(&bytes).to_hex();
            self.put_create(
                &self.v2_key(&[
                    "catalog",
                    "registry",
                    &format!("{}.json", Self::object_name(&client)),
                ]),
                client.as_bytes().to_vec(),
            )?;
            self.put_create(
                &self.v2_key(&[
                    "catalog",
                    "scopes",
                    &format!("{}.json", Self::object_name(&scope)),
                ]),
                scope.as_bytes().to_vec(),
            )?;
            self.put_create(
                &self.v2_key(&[
                    "catalog",
                    "clients",
                    &Self::object_name(&client),
                    &Self::object_name(&scope),
                    &format!(
                        "{first_counter:020}-{last_counter:020}-{digest}.json"
                    ),
                ]),
                bytes,
            )?;
        }
        Ok(CommitBatchResult {
            visible_commits: batch
                .commits
                .into_iter()
                .map(|commit| commit.id)
                .collect(),
        })
    }

    fn lookup_chunks(
        &self,
        hashes: Vec<String>,
    ) -> Result<Vec<ChunkLocation>, SyncError> {
        let wanted: HashSet<_> = hashes.into_iter().collect();
        let ids = if let Some(ids) = self
            .v2_pack_index_ids
            .lock()
            .expect("v2 pack index ids lock poisoned")
            .clone()
        {
            ids
        } else {
            let mut ids: Vec<_> = self
                .list_paths(&self.v2_key(&["pack-indexes"]))?
                .into_iter()
                .filter_map(|path| path.filename().map(str::to_owned))
                .collect();
            ids.sort();
            *self
                .v2_pack_index_ids
                .lock()
                .expect("v2 pack index ids lock poisoned") = Some(ids.clone());
            ids
        };
        let mut found = HashMap::new();
        for id in ids {
            let bytes = if let Some(bytes) = self
                .v2_pack_index_cache
                .lock()
                .expect("v2 pack index cache lock poisoned")
                .get(&id)
                .cloned()
            {
                bytes
            } else {
                let bytes = self
                    .get_opt(&self.v2_key(&["pack-indexes", &id]))?
                    .ok_or_else(|| {
                        io(format!("v2 pack index not found: {id}"))
                    })?;
                self.v2_pack_index_cache
                    .lock()
                    .expect("v2 pack index cache lock poisoned")
                    .insert(id.clone(), bytes.clone());
                bytes
            };
            let index: rollforward::binary::PackIndex =
                serde_json::from_slice(&bytes).map_err(|error| {
                    SyncError::SerdeError { msg: error.to_string() }
                })?;
            for chunk in index.chunks {
                if wanted.contains(&chunk.hash) {
                    found.entry(chunk.hash.clone()).or_insert(ChunkLocation {
                        hash: chunk.hash,
                        pack_id: index.pack_id.clone(),
                        offset: chunk.offset,
                        length: chunk.length,
                    });
                }
            }
        }
        Ok(found.into_values().collect())
    }

    fn read_ranges(
        &self,
        ranges: Vec<PackRange>,
    ) -> Result<Vec<RangeData>, SyncError> {
        let store = self.store.clone();
        let jobs: Vec<_> = ranges
            .into_iter()
            .map(|range| (self.v2_key(&["packs", &range.pack_id]), range))
            .collect();
        self.block(async move {
            let mut reads =
                futures::stream::iter(jobs.into_iter().map(|(key, range)| {
                    let store = store.clone();
                    async move {
                        let start =
                            usize::try_from(range.offset).map_err(io)?;
                        let end = start
                            .checked_add(range.length as usize)
                            .ok_or_else(|| io("pack range overflow"))?;
                        let data = store
                            .get_range(&key, start..end)
                            .await
                            .map_err(io)?
                            .to_vec();
                        if blake3::hash(&data).to_hex().as_str() != range.hash
                        {
                            return Err(SyncError::Corrupt {
                                hash: range.hash,
                            });
                        }
                        Ok(RangeData { hash: range.hash, data })
                    }
                }))
                .buffer_unordered(32);
            let mut output = Vec::new();
            while let Some(result) = reads.next().await {
                output.push(result?);
            }
            Ok(output)
        })
    }

    fn write_acknowledgements(
        &self,
        acknowledgements: Vec<ResourceAck>,
    ) -> Result<(), SyncError> {
        let store = self.store.clone();
        let jobs: Vec<_> = acknowledgements
            .into_iter()
            .map(|ack| {
                let id = format!(
                    "{}\0{}\0{}",
                    ack.client_id,
                    ack.resource.scope_id,
                    ack.resource.resource_id
                );
                Ok::<_, SyncError>((
                    self.v2_key(&[
                        "acknowledgements",
                        &Self::object_name(&id),
                    ]),
                    serde_json::to_vec(&ack).map_err(|error| {
                        SyncError::SerdeError { msg: error.to_string() }
                    })?,
                ))
            })
            .collect::<Result<_, _>>()?;
        self.block(async move {
            let mut writes =
                futures::stream::iter(jobs.into_iter().map(|(key, bytes)| {
                    let store = store.clone();
                    async move { store.put(&key, bytes.into()).await }
                }))
                .buffer_unordered(32);
            while let Some(result) = writes.next().await {
                result?;
            }
            Ok::<_, ObjectStoreError>(())
        })
        .map_err(io)
    }

    fn list_acknowledgements(
        &self,
        resources: Vec<ResourceKey>,
    ) -> Result<Vec<ResourceAck>, SyncError> {
        let wanted: HashSet<_> = resources.into_iter().collect();
        let mut output = Vec::new();
        for path in self.list_paths(&self.v2_key(&["acknowledgements"]))? {
            if let Some(bytes) = self.get_opt(&path)? {
                let ack: ResourceAck = serde_json::from_slice(&bytes)
                    .map_err(|error| SyncError::SerdeError {
                        msg: error.to_string(),
                    })?;
                if wanted.is_empty() || wanted.contains(&ack.resource) {
                    output.push(ack);
                }
            }
        }
        Ok(output)
    }

    fn list_pack_ids(&self) -> Result<Vec<String>, SyncError> {
        Ok(self
            .list_paths(&self.v2_key(&["packs"]))?
            .into_iter()
            .filter_map(|path| path.filename().map(str::to_owned))
            .collect())
    }

    fn delete_pack_objects(
        &self,
        pack_ids: Vec<String>,
    ) -> Result<(), SyncError> {
        for id in pack_ids {
            let _ = self.block(async {
                self.store.delete(&self.v2_key(&["packs", &id])).await
            });
            let _ = self.block(async {
                self.store.delete(&self.v2_key(&["pack-indexes", &id])).await
            });
            self.v2_pack_index_cache
                .lock()
                .expect("v2 pack index cache lock poisoned")
                .remove(&id);
        }
        Ok(())
    }

    fn compact_catalog(
        &self,
        compaction: CatalogCompaction,
    ) -> Result<(), SyncError> {
        let scope_hash = Self::object_name(&compaction.snapshot.scope_id);
        let generation = compaction.snapshot.generation;
        let snapshot_key = self.v2_key(&[
            "catalog",
            "snapshots",
            &scope_hash,
            &format!("{generation:020}.json"),
        ]);
        self.put_create(
            &snapshot_key,
            serde_json::to_vec(&compaction.snapshot).map_err(|error| {
                SyncError::SerdeError { msg: error.to_string() }
            })?,
        )?;
        // Publishing latest is the visibility boundary. Covered segments and
        // commits are deleted only after readers can discover the snapshot.
        let latest_key =
            self.v2_key(&["catalog", "snapshots", &scope_hash, "latest.json"]);
        self.block(async {
            self.store
                .put(
                    &latest_key,
                    serde_json::to_vec(&generation).map_err(io)?.into(),
                )
                .await
                .map_err(io)
        })?;

        let covered: BTreeMap<_, _> = compaction
            .snapshot
            .cursors
            .iter()
            .map(|cursor| (cursor.client_id.clone(), cursor.counter))
            .collect();
        for (client, counter) in covered {
            let prefix = self.v2_key(&[
                "catalog",
                "clients",
                &Self::object_name(&client),
                &scope_hash,
            ]);
            for path in self.list_paths(&prefix)? {
                let last = path
                    .filename()
                    .and_then(|name| name.split('-').nth(1))
                    .and_then(|value| value.parse::<u64>().ok());
                if last.is_some_and(|last| last <= counter) {
                    self.block(async { self.store.delete(&path).await })
                        .map_err(io)?;
                }
            }
        }
        for id in compaction.obsolete_commit_ids {
            let key = self.v2_key(&["commits", &Self::object_name(&id)]);
            self.block(async { self.store.delete(&key).await }).map_err(io)?;
        }

        let snapshot_prefix =
            self.v2_key(&["catalog", "snapshots", &scope_hash]);
        let mut generations: Vec<_> = self
            .list_paths(&snapshot_prefix)?
            .into_iter()
            .filter(|path| path.filename() != Some("latest.json"))
            .collect();
        generations.sort_by(|left, right| right.as_ref().cmp(left.as_ref()));
        for path in generations.into_iter().skip(2) {
            self.block(async { self.store.delete(&path).await })
                .map_err(io)?;
        }
        Ok(())
    }
}

#[cfg(any())]
mod tests {
    use super::*;
    use object_store::local::LocalFileSystem;
    use rollforward::types::ChangeType;
    use rollforward::{CatalogSnapshot, ResourceChange};
    use std::sync::Arc;

    fn entry(seq: u64, client: &str) -> OpLogEntry {
        OpLogEntry {
            sequence: seq,
            client_id: client.to_owned(),
            timestamp: 0,
            change_type: ChangeType::TextDelta {
                delta: vec![u8::try_from(seq & 0xff).unwrap_or(0)],
            },
        }
    }

    /// Build an `S3Remote` over a `LocalFileSystem` rooted at a temp dir.
    fn temp_remote(dir: &std::path::Path) -> S3Remote {
        let store = Arc::new(
            LocalFileSystem::new_with_prefix(dir).expect("local store"),
        );
        S3Remote::new(store, "lib", Handle::current())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn oplog_round_trips() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        let e = entry(1, "a");
        remote.put_oplog("f1".into(), e.clone()).unwrap();

        let items = remote.list_oplogs("f1".into()).unwrap();
        assert_eq!(items.len(), 1);
        let raw = remote
            .get_oplog("f1".into(), items[0].remote_path.clone())
            .unwrap();
        let decoded: OpLogEntry = serde_json::from_slice(&raw).unwrap();
        assert_eq!(decoded, e);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_files_preserves_nested_file_id() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        remote
            .put_oplog("item-1/figures/nested/plot.png".into(), entry(1, "a"))
            .unwrap();
        assert_eq!(
            remote.list_files().unwrap(),
            vec!["item-1/figures/nested/plot.png".to_owned()]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn inventory_tracks_files_created_after_migration() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        assert!(remote.list_files().unwrap().is_empty());
        remote.put_oplog("item/new.bin".into(), entry(1, "a")).unwrap();
        // Simulate a process exit before the end-of-run snapshot is written:
        // the pre-oplog immutable marker must already make the file visible.
        drop(remote);
        let remote = temp_remote(dir.path());
        assert_eq!(
            remote.list_files().unwrap(),
            vec!["item/new.bin".to_owned()]
        );
        drop(remote);
        let reopened = temp_remote(dir.path());
        assert_eq!(
            reopened.list_files().unwrap(),
            vec!["item/new.bin".to_owned()]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cas_rejects_duplicate_sequence() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        remote.put_oplog_cas("f1".into(), entry(5, "a")).unwrap();

        let err =
            remote.put_oplog_cas("f1".into(), entry(5, "b")).unwrap_err();
        assert!(matches!(err, SyncError::Conflict { sequence: 5 }));
        assert_eq!(remote.list_oplogs("f1".into()).unwrap().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cas_allows_distinct_sequences() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        remote.put_oplog_cas("f1".into(), entry(1, "a")).unwrap();
        remote.put_oplog_cas("f1".into(), entry(2, "a")).unwrap();
        assert_eq!(remote.list_oplogs("f1".into()).unwrap().len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn packs_round_trip_range_and_list() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        remote.put_pack("p1".into(), b"HELLOWORLD".to_vec()).unwrap();
        remote.put_pack("p1".into(), b"HELLOWORLD".to_vec()).unwrap(); // idempotent
        // Range reads pull a chunk-sized slice out of the pack.
        assert_eq!(
            remote.get_pack_range("p1".into(), 0, 5).unwrap(),
            b"HELLO"
        );
        assert_eq!(
            remote.get_pack_range("p1".into(), 5, 5).unwrap(),
            b"WORLD"
        );
        assert_eq!(remote.list_packs().unwrap(), vec!["p1".to_string()]);
        remote.delete_pack("p1".into()).unwrap();
        assert!(remote.list_packs().unwrap().is_empty());
        remote.delete_pack("p1".into()).unwrap(); // absent = no-op
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pack_indexes_round_trip_and_list() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        remote.put_pack_index("p1".into(), b"idx".to_vec()).unwrap();
        assert_eq!(remote.get_pack_index("p1".into()).unwrap(), b"idx");
        assert_eq!(
            remote.list_pack_indexes().unwrap(),
            vec!["p1".to_string()]
        );
        remote.delete_pack_index("p1".into()).unwrap();
        assert!(remote.list_pack_indexes().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_files_reports_only_file_ids_with_oplogs() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        // A file id gets an oplog; the reserved global stores get objects too.
        remote.put_oplog("f1".into(), entry(1, "a")).unwrap();
        remote.put_pack("p1".into(), b"x".to_vec()).unwrap();
        remote.put_status("a".into(), 1).unwrap();
        // Only the file id with oplog history is listed.
        assert_eq!(remote.list_files().unwrap(), vec!["f1".to_string()]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn baselines_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        remote.put_baseline("f1".into(), 110, b"snap".to_vec()).unwrap();
        assert_eq!(
            remote.get_baseline("f1".into(), 110).unwrap().unwrap(),
            b"snap"
        );
        assert!(remote.get_baseline("f1".into(), 999).unwrap().is_none());
        assert_eq!(remote.list_baselines("f1".into()).unwrap(), vec![110]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn statuses_round_trip_and_min() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        remote.put_status("a".into(), 120).unwrap();
        remote.put_status("b".into(), 115).unwrap();
        let mut statuses = remote.list_statuses().unwrap();
        statuses.sort_by(|x, y| x.client_id.cmp(&y.client_id));
        assert_eq!(statuses[0].last_synced_sequence, 120);
        assert_eq!(statuses[1].last_synced_sequence, 115);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn v2_snapshot_replaces_covered_segments_without_losing_head() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        let key = ResourceKey::new("scope", "nested/file.bin");
        let first = Commit::create(
            key.clone(),
            Vec::new(),
            "client".into(),
            1,
            ResourceChange::Delete,
        );
        let second = Commit::create(
            key.clone(),
            vec![first.id.clone()],
            "client".into(),
            2,
            ResourceChange::Delete,
        );
        remote
            .commit_batch(CommitBatch {
                commits: vec![first.clone(), second.clone()],
                ..CommitBatch::default()
            })
            .unwrap();
        let delta = remote
            .scan_catalog(CatalogScanRequest {
                scopes: vec!["scope".into()],
                cursors: Vec::new(),
            })
            .unwrap();
        assert_eq!(delta.events.len(), 2);

        remote
            .compact_catalog(CatalogCompaction {
                snapshot: CatalogSnapshot {
                    generation: 1,
                    scope_id: "scope".into(),
                    events: vec![CatalogEvent {
                        client_id: "client".into(),
                        counter: 2,
                        commit_id: second.id.clone(),
                        resource: key,
                    }],
                    cursors: delta.cursors,
                },
                obsolete_commit_ids: vec![first.id.clone()],
            })
            .unwrap();

        let fresh = remote
            .scan_catalog(CatalogScanRequest {
                scopes: vec!["scope".into()],
                cursors: Vec::new(),
            })
            .unwrap();
        assert_eq!(fresh.events.len(), 1);
        assert_eq!(fresh.events[0].commit_id, second.id);
        assert!(remote.load_commits(vec![first.id]).is_err());
    }
}
