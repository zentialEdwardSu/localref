//! Rollforward v2 integration smoke tests through the S3 object-store adapter.

#[path = "../src/s3_remote.rs"]
mod s3_remote;

use object_store::ObjectStore;
use object_store::local::LocalFileSystem;
use rollforward::{
    ApplyStatus, EngineEvent, EngineEventListenerV2, LocalApplyResult,
    LocalMutation, LocalReplica, LocalResource, MutationPrecondition,
    RedbRuntimeStore, ReplicaState, ResourceContent, ResourceKey,
    ResourceKind, SyncRequest, SyncRuntime,
};
use s3_remote::S3Remote;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

#[derive(Default)]
struct MemoryReplica {
    files: Mutex<BTreeMap<ResourceKey, (Vec<u8>, u64)>>,
}

impl MemoryReplica {
    fn put(&self, key: ResourceKey, data: &[u8]) {
        let mut files = self.files.lock().unwrap();
        let version = files.get(&key).map_or(1, |(_, version)| version + 1);
        files.insert(key, (data.to_vec(), version));
    }
}

impl LocalReplica for MemoryReplica {
    fn list_resources(
        &self,
        scopes: Vec<String>,
    ) -> Result<Vec<LocalResource>, rollforward::SyncError> {
        Ok(self
            .files
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, _)| {
                scopes.is_empty() || scopes.contains(&key.scope_id)
            })
            .map(|(key, (data, version))| LocalResource {
                key: key.clone(),
                kind: ResourceKind::Binary,
                state: ReplicaState::present(
                    blake3::hash(data).to_hex().to_string(),
                    data.len() as u64,
                    version.to_string(),
                ),
            })
            .collect())
    }

    fn read_resources(
        &self,
        keys: Vec<ResourceKey>,
    ) -> Result<Vec<ResourceContent>, rollforward::SyncError> {
        let files = self.files.lock().unwrap();
        Ok(keys
            .into_iter()
            .filter_map(|key| {
                files.get(&key).map(|(data, version)| ResourceContent {
                    key,
                    kind: ResourceKind::Binary,
                    version_token: version.to_string(),
                    data: data.clone(),
                })
            })
            .collect())
    }

    fn apply_mutations(
        &self,
        mutations: Vec<LocalMutation>,
    ) -> Result<Vec<LocalApplyResult>, rollforward::SyncError> {
        let mut files = self.files.lock().unwrap();
        Ok(mutations
            .into_iter()
            .map(|mutation| {
                let key = match &mutation {
                    LocalMutation::WritePresent { key, .. }
                    | LocalMutation::ApplyDelete { key, .. }
                    | LocalMutation::CreateCopy { key, .. } => key.clone(),
                };
                let precondition = match &mutation {
                    LocalMutation::WritePresent { precondition, .. }
                    | LocalMutation::ApplyDelete { precondition, .. } => {
                        Some(precondition)
                    }
                    LocalMutation::CreateCopy { .. } => None,
                };
                let matches = precondition.map_or(
                    !files.contains_key(&key),
                    |condition| match condition {
                        MutationPrecondition::Missing => {
                            !files.contains_key(&key)
                        }
                        MutationPrecondition::Version { version_token } => {
                            files.get(&key).is_some_and(|(_, version)| {
                                version.to_string() == *version_token
                            })
                        }
                    },
                );
                if !matches {
                    return LocalApplyResult {
                        key,
                        status: ApplyStatus::PreconditionFailed,
                        version_token: None,
                        error: None,
                    };
                }
                let version =
                    files.get(&key).map_or(1, |(_, version)| version + 1);
                match mutation {
                    LocalMutation::WritePresent { data, .. }
                    | LocalMutation::CreateCopy { data, .. } => {
                        files.insert(key.clone(), (data, version));
                    }
                    LocalMutation::ApplyDelete { .. } => {
                        files.remove(&key);
                    }
                }
                LocalApplyResult {
                    key,
                    status: ApplyStatus::Applied,
                    version_token: Some(version.to_string()),
                    error: None,
                }
            })
            .collect())
    }
}

struct NoEvents;
impl EngineEventListenerV2 for NoEvents {
    fn on_events(&self, _: Vec<EngineEvent>) {}
}

fn runtime(
    root: &std::path::Path,
    client: &str,
    remote: Arc<S3Remote>,
    local: Arc<MemoryReplica>,
) -> SyncRuntime {
    SyncRuntime::with_backends(
        client.into(),
        Arc::new(
            RedbRuntimeStore::open(root.join(format!("{client}.redb")))
                .unwrap(),
        ),
        remote,
        local,
        Arc::new(NoEvents),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_s3_adapter_converges_nested_binary() {
    let root = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(root.path().join("remote")).unwrap();
    let store: Arc<dyn ObjectStore> = Arc::new(
        LocalFileSystem::new_with_prefix(root.path().join("remote")).unwrap(),
    );
    let remote = Arc::new(S3Remote::new(store, "library", Handle::current()));
    let key = ResourceKey::new("item", "nested/file.bin");
    let first = Arc::new(MemoryReplica::default());
    first.put(key.clone(), b"first");
    runtime(root.path(), "a", remote.clone(), first)
        .reconcile(SyncRequest { scopes: vec!["item".into()] })
        .unwrap();

    let second = Arc::new(MemoryReplica::default());
    let engine = runtime(root.path(), "b", remote, second.clone());
    assert_eq!(
        engine
            .reconcile(SyncRequest { scopes: vec!["item".into()] })
            .unwrap()
            .downloaded,
        1
    );
    assert_eq!(second.files.lock().unwrap()[&key].0, b"first");
}
