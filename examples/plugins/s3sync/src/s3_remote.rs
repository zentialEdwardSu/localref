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
//! <prefix>/chunks/<hash>
//! <prefix>/clients_status/<client>.status
//! ```
//!
//! ## Async/sync bridge
//! `RemoteStorage`'s methods are synchronous but `object_store` is async. The
//! plugin runs on a multi-thread Tokio runtime, so each method bridges with
//! [`tokio::task::block_in_place`] over a stored runtime [`Handle`] — valid
//! because the engine calls these from a normal worker thread, never from
//! inside another `block_on`.

use futures::StreamExt as _;
use object_store::path::Path as ObjPath;
use object_store::{Error as ObjectStoreError, ObjectStore, PutMode, PutOptions};
use rollforward::types::{ClientStatus, OpLogEntry, RemoteLogItem, SyncError};
use rollforward::RemoteStorage;
use std::sync::Arc;
use tokio::runtime::Handle;

/// The `.oplog` filename suffix, matching rollforward's `oplog` module.
const OPLOG_EXT: &str = "oplog";

/// A [`RemoteStorage`] backed by any [`ObjectStore`] (S3, local folder, …).
pub struct S3Remote {
    /// The underlying object store (e.g. `AmazonS3` or `LocalFileSystem`).
    store: Arc<dyn ObjectStore>,
    /// Key prefix under which all objects for this library live.
    prefix: String,
    /// Handle to the multi-thread runtime used to drive async store calls.
    handle: Handle,
}

impl std::fmt::Debug for S3Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Remote").field("prefix", &self.prefix).finish_non_exhaustive()
    }
}

/// Map any `object_store` error into a rollforward [`SyncError::IoError`].
fn io(e: impl std::fmt::Display) -> SyncError {
    SyncError::IoError { msg: e.to_string() }
}

/// `{seq}_{client}.oplog` — the oplog object filename for a sequence/client.
fn oplog_name(sequence: u64, client_id: &str) -> String {
    format!("{sequence}_{client_id}.{OPLOG_EXT}")
}

/// Parse `{seq}_{client}.oplog` back into `(sequence, client_id)`.
fn parse_oplog_name(name: &str) -> Option<(u64, String)> {
    let stem = name.strip_suffix(&format!(".{OPLOG_EXT}"))?;
    let (seq, client) = stem.split_once('_')?;
    Some((seq.parse().ok()?, client.to_owned()))
}

impl S3Remote {
    /// Build a remote over `store`, rooting every key under `prefix`.
    pub fn new(store: Arc<dyn ObjectStore>, prefix: impl Into<String>, handle: Handle) -> Self {
        let mut prefix = prefix.into();
        while prefix.ends_with('/') {
            prefix.pop();
        }
        Self { store, prefix, handle }
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

    /// Run an async store operation to completion on the stored runtime.
    fn block<F: Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.handle.block_on(fut))
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
}

impl RemoteStorage for S3Remote {
    fn list_oplogs(&self, file_id: String) -> Result<Vec<RemoteLogItem>, SyncError> {
        let dir = self.key(&[&file_id, "oplogs"]);
        let mut out = Vec::new();
        for name in self.list_names(&dir)? {
            if let Some((sequence, client_id)) = parse_oplog_name(&name) {
                out.push(RemoteLogItem { sequence, client_id, remote_path: name });
            }
        }
        Ok(out)
    }

    fn put_oplog(&self, file_id: String, entry: OpLogEntry) -> Result<(), SyncError> {
        let key = self.key(&[&file_id, "oplogs", &oplog_name(entry.sequence, &entry.client_id)]);
        let bytes = serde_json::to_vec(&entry).map_err(|e| SyncError::SerdeError { msg: e.to_string() })?;
        self.block(async { self.store.put(&key, bytes.into()).await }).map_err(io)?;
        Ok(())
    }

    fn put_oplog_cas(&self, file_id: String, entry: OpLogEntry) -> Result<(), SyncError> {
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
        let key = self.key(&[&file_id, "oplogs", &oplog_name(entry.sequence, &entry.client_id)]);
        let bytes = serde_json::to_vec(&entry).map_err(|e| SyncError::SerdeError { msg: e.to_string() })?;
        let opts = PutOptions { mode: PutMode::Create, ..Default::default() };
        match self.block(async { self.store.put_opts(&key, bytes.into(), opts).await }) {
            Ok(_) => Ok(()),
            Err(ObjectStoreError::AlreadyExists { .. }) => {
                Err(SyncError::Conflict { sequence: entry.sequence })
            }
            Err(e) => Err(io(e)),
        }
    }

    fn get_oplog(&self, file_id: String, remote_path: String) -> Result<Vec<u8>, SyncError> {
        let key = self.key(&[&file_id, "oplogs", &remote_path]);
        self.get_opt(&key)?.ok_or_else(|| io(format!("oplog not found: {remote_path}")))
    }

    fn delete_oplog(&self, file_id: String, remote_path: String) -> Result<(), SyncError> {
        let key = self.key(&[&file_id, "oplogs", &remote_path]);
        match self.block(async { self.store.delete(&key).await }) {
            Ok(()) | Err(ObjectStoreError::NotFound { .. }) => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    fn put_chunk(&self, hash: String, data: Vec<u8>) -> Result<(), SyncError> {
        let key = self.key(&["chunks", &hash]);
        // Content-addressed: identical bytes under the same hash, so an existing
        // object is fine — an unconditional put is idempotent here.
        self.block(async { self.store.put(&key, data.into()).await }).map_err(io)?;
        Ok(())
    }

    fn get_chunk(&self, hash: String) -> Result<Vec<u8>, SyncError> {
        let key = self.key(&["chunks", &hash]);
        self.get_opt(&key)?.ok_or_else(|| io(format!("chunk not found: {hash}")))
    }

    fn delete_chunk(&self, hash: String) -> Result<(), SyncError> {
        let key = self.key(&["chunks", &hash]);
        match self.block(async { self.store.delete(&key).await }) {
            Ok(()) | Err(ObjectStoreError::NotFound { .. }) => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    fn list_chunks(&self) -> Result<Vec<String>, SyncError> {
        self.list_names(&self.key(&["chunks"]))
    }

    fn put_baseline(&self, file_id: String, seq: u64, data: Vec<u8>) -> Result<(), SyncError> {
        let key = self.key(&[&file_id, "baselines", &baseline_name(seq)]);
        self.block(async { self.store.put(&key, data.into()).await }).map_err(io)?;
        Ok(())
    }

    fn get_baseline(&self, file_id: String, seq: u64) -> Result<Option<Vec<u8>>, SyncError> {
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

    fn put_status(&self, client_id: String, last_synced_sequence: u64) -> Result<(), SyncError> {
        let key = self.key(&["clients_status", &format!("{client_id}.status")]);
        let body = serde_json::json!({ "last_synced_sequence": last_synced_sequence });
        let bytes = serde_json::to_vec(&body).map_err(|e| SyncError::SerdeError { msg: e.to_string() })?;
        self.block(async { self.store.put(&key, bytes.into()).await }).map_err(io)?;
        Ok(())
    }

    fn list_statuses(&self) -> Result<Vec<ClientStatus>, SyncError> {
        let dir = self.key(&["clients_status"]);
        let mut out = Vec::new();
        for name in self.list_names(&dir)? {
            let Some(client) = name.strip_suffix(".status") else { continue };
            let key = self.key(&["clients_status", &name]);
            let Some(bytes) = self.get_opt(&key)? else { continue };
            let val: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| SyncError::SerdeError { msg: e.to_string() })?;
            let seq = val
                .get("last_synced_sequence")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| SyncError::SerdeError { msg: format!("bad status file: {name}") })?;
            out.push(ClientStatus { client_id: client.to_owned(), last_synced_sequence: seq });
        }
        Ok(out)
    }
}

/// `baseline_<seq>.zst`
fn baseline_name(seq: u64) -> String {
    format!("baseline_{seq}.zst")
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::local::LocalFileSystem;
    use rollforward::types::ChangeType;
    use std::sync::Arc;

    fn entry(seq: u64, client: &str) -> OpLogEntry {
        OpLogEntry {
            sequence: seq,
            client_id: client.to_owned(),
            timestamp: 0,
            change_type: ChangeType::TextDelta { delta: vec![u8::try_from(seq & 0xff).unwrap_or(0)] },
        }
    }

    /// Build an `S3Remote` over a `LocalFileSystem` rooted at a temp dir.
    fn temp_remote(dir: &std::path::Path) -> S3Remote {
        let store = Arc::new(LocalFileSystem::new_with_prefix(dir).expect("local store"));
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
        let raw = remote.get_oplog("f1".into(), items[0].remote_path.clone()).unwrap();
        let decoded: OpLogEntry = serde_json::from_slice(&raw).unwrap();
        assert_eq!(decoded, e);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cas_rejects_duplicate_sequence() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        remote.put_oplog_cas("f1".into(), entry(5, "a")).unwrap();

        let err = remote.put_oplog_cas("f1".into(), entry(5, "b")).unwrap_err();
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
    async fn chunks_round_trip_and_list() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        remote.put_chunk("h1".into(), b"hello".to_vec()).unwrap();
        remote.put_chunk("h1".into(), b"hello".to_vec()).unwrap();
        assert_eq!(remote.get_chunk("h1".into()).unwrap(), b"hello");
        assert_eq!(remote.list_chunks().unwrap(), vec!["h1".to_string()]);
        remote.delete_chunk("h1".into()).unwrap();
        assert!(remote.list_chunks().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn baselines_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let remote = temp_remote(dir.path());
        remote.put_baseline("f1".into(), 110, b"snap".to_vec()).unwrap();
        assert_eq!(remote.get_baseline("f1".into(), 110).unwrap().unwrap(), b"snap");
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
}
