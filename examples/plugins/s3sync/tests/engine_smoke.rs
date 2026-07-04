//! End-to-end smoke test: two engines over one shared object-store remote,
//! exercising binary convergence and the `KeepBoth` conflict callback — the
//! same shape rollforward's own `tests/sync.rs` uses, but through our
//! `S3Remote` (over a `LocalFileSystem`) to prove the backend integrates.
//!
//! The `S3Remote` type lives in the binary crate (`src/`), so this integration
//! test includes the module source directly rather than importing it.

#[path = "../src/s3_remote.rs"]
mod s3_remote;

use object_store::local::LocalFileSystem;
use object_store::ObjectStore;
use rollforward::types::{BinaryConflictPolicy, EngineNotificationListener};
use rollforward::{RedbStore, RemoteStorage, SyncEngine};
use s3_remote::S3Remote;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

/// A listener that records conflict-copy requests and content updates.
#[derive(Default)]
struct TestListener {
    /// Count of `on_conflict_copy_requested` calls.
    conflicts: AtomicUsize,
    /// File ids passed to `on_file_content_updated`.
    updated: Mutex<Vec<String>>,
}

impl EngineNotificationListener for TestListener {
    fn on_file_content_updated(&self, file_id: String) {
        self.updated.lock().unwrap().push(file_id);
    }
    fn on_conflict_copy_requested(&self, _file_id: String, _suggested: String) {
        self.conflicts.fetch_add(1, Ordering::SeqCst);
    }
}

/// Build an `S3Remote` over a `LocalFileSystem` rooted at `dir`.
fn remote(dir: &std::path::Path) -> Arc<S3Remote> {
    let store: Arc<dyn ObjectStore> =
        Arc::new(LocalFileSystem::new_with_prefix(dir).expect("local store"));
    Arc::new(S3Remote::new(store, "lib", Handle::current()))
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_converge_on_binary() {
    let remote_dir = tempfile::TempDir::new().unwrap();
    let db_a = tempfile::TempDir::new().unwrap();
    let db_b = tempfile::TempDir::new().unwrap();
    let shared = remote(remote_dir.path());

    let la = Arc::new(TestListener::default());
    let lb = Arc::new(TestListener::default());
    let engine_a = SyncEngine::with_backends(
        "clientA",
        Arc::new(RedbStore::open(db_a.path().join("a.redb")).unwrap()),
        shared.clone(),
        la.clone(),
        BinaryConflictPolicy::KeepBoth,
    );
    let engine_b = SyncEngine::with_backends(
        "clientB",
        Arc::new(RedbStore::open(db_b.path().join("b.redb")).unwrap()),
        shared.clone(),
        lb.clone(),
        BinaryConflictPolicy::KeepBoth,
    );

    // A publishes v1; B syncs and sees the same manifest.
    engine_a.modify_binary("doc.bin".into(), b"hello world".to_vec()).unwrap();
    engine_b.sync("doc.bin".into()).unwrap();
    assert_eq!(
        engine_a.get_manifest("doc.bin".into()).unwrap(),
        engine_b.get_manifest("doc.bin".into()).unwrap(),
        "B should converge to A's content after sync"
    );

    // Reassembling B's manifest from the shared remote yields A's bytes.
    let manifest = engine_b.get_manifest("doc.bin".into()).unwrap();
    let mut content = Vec::new();
    for hash in &manifest {
        content.extend_from_slice(&shared.get_chunk(hash.clone()).unwrap());
    }
    assert_eq!(content, b"hello world");
}

#[tokio::test(flavor = "multi_thread")]
async fn divergent_binary_edit_triggers_conflict_copy() {
    let remote_dir = tempfile::TempDir::new().unwrap();
    let db_a = tempfile::TempDir::new().unwrap();
    let db_b = tempfile::TempDir::new().unwrap();
    let shared = remote(remote_dir.path());

    let la = Arc::new(TestListener::default());
    let lb = Arc::new(TestListener::default());
    let engine_a = SyncEngine::with_backends(
        "clientA",
        Arc::new(RedbStore::open(db_a.path().join("a.redb")).unwrap()),
        shared.clone(),
        la.clone(),
        BinaryConflictPolicy::KeepBoth,
    );
    let engine_b = SyncEngine::with_backends(
        "clientB",
        Arc::new(RedbStore::open(db_b.path().join("b.redb")).unwrap()),
        shared.clone(),
        lb.clone(),
        BinaryConflictPolicy::KeepBoth,
    );

    // Shared common ancestor at seq 1.
    engine_a.modify_binary("doc.bin".into(), b"base".to_vec()).unwrap();
    engine_b.sync("doc.bin".into()).unwrap();

    // Force a genuine fork: both publish a divergent edit at the same next
    // sequence via the non-CAS put (mirrors rollforward's own fork injection).
    let fork_seq = engine_a.head("doc.bin".into()) + 1;
    let mk = |client: &str, bytes: &[u8]| {
        let chunks = rollforward::binary::chunk_data(bytes);
        for (info, data) in &chunks {
            shared.put_chunk(info.hash.clone(), data.clone()).unwrap();
        }
        rollforward::types::OpLogEntry {
            sequence: fork_seq,
            client_id: client.to_owned(),
            timestamp: 0,
            change_type: rollforward::types::ChangeType::BinarySnapshot {
                chunk_hashes: chunks.iter().map(|(c, _)| c.hash.clone()).collect(),
            },
        }
    };
    shared.put_oplog("doc.bin".into(), mk("clientA", b"alpha edit")).unwrap();
    shared.put_oplog("doc.bin".into(), mk("clientB", b"beta edit")).unwrap();

    // Syncing sees the forked tip and must request a keep-both copy.
    engine_a.sync("doc.bin".into()).unwrap();
    assert!(
        la.conflicts.load(Ordering::SeqCst) >= 1,
        "a divergent binary fork should trigger on_conflict_copy_requested"
    );
}
