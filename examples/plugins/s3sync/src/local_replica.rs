//! Localref attachment adapter for the rollforward v2 runtime.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use localref_plugin_sdk::LocalrefClient;
use rollforward::{
    ApplyStatus, LocalApplyResult, LocalMutation, LocalReplica, LocalResource,
    MutationPrecondition, ReplicaState, ResourceContent, ResourceKey,
    ResourceKind, SyncError,
};
use tokio::runtime::Handle;

const NS: &str = "s3sync";

fn error(message: impl Into<String>) -> SyncError {
    SyncError::IoError { msg: message.into() }
}

fn safe_relative(value: &str) -> Result<&Path, SyncError> {
    let path = Path::new(value);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(error(format!("unsafe resource path: {value}")));
    }
    Ok(path)
}

fn version_token(path: &Path) -> Result<String, SyncError> {
    let metadata =
        std::fs::metadata(path).map_err(|e| error(e.to_string()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    Ok(format!("{}:{modified}", metadata.len()))
}

pub struct LocalrefReplica {
    client: LocalrefClient,
    handle: Handle,
    library_root: PathBuf,
    temp_root: PathBuf,
    paths: Mutex<HashMap<ResourceKey, PathBuf>>,
    scope_roots: Mutex<HashMap<String, PathBuf>>,
}

impl LocalrefReplica {
    pub fn new(
        client: LocalrefClient,
        handle: Handle,
        library_root: PathBuf,
        temp_root: PathBuf,
    ) -> Self {
        Self {
            client,
            handle,
            library_root,
            temp_root,
            paths: Mutex::new(HashMap::new()),
            scope_roots: Mutex::new(HashMap::new()),
        }
    }

    fn path_for(&self, key: &ResourceKey) -> Result<PathBuf, SyncError> {
        if let Some(path) = self
            .paths
            .lock()
            .expect("replica paths lock poisoned")
            .get(key)
            .cloned()
        {
            return Ok(path);
        }
        let root = self
            .scope_roots
            .lock()
            .expect("scope roots lock poisoned")
            .get(&key.scope_id)
            .cloned()
            .ok_or_else(|| {
                error(format!("unknown local scope {}", key.scope_id))
            })?;
        Ok(root.join(safe_relative(&key.resource_id)?))
    }

    fn matches_precondition(
        &self,
        path: &Path,
        precondition: &MutationPrecondition,
    ) -> bool {
        match precondition {
            MutationPrecondition::Missing => !path.exists(),
            MutationPrecondition::Version { version_token: expected } => {
                version_token(path).is_ok_and(|actual| actual == *expected)
            }
        }
    }

    fn add_at(
        &self,
        key: &ResourceKey,
        data: &[u8],
    ) -> Result<String, SyncError> {
        let temp = self
            .temp_root
            .join("runtime-v2")
            .join(blake3::hash(key.encoded().as_bytes()).to_hex().to_string());
        if let Some(parent) = temp.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| error(e.to_string()))?;
        }
        std::fs::write(&temp, data).map_err(|e| error(e.to_string()))?;
        let temp_text = temp.to_string_lossy().into_owned();
        let result = self.handle.block_on(self.client.add_file_at(
            &key.scope_id,
            &temp_text,
            &key.resource_id,
        ));
        let _ = std::fs::remove_file(&temp);
        result.map_err(|e| error(e.to_string()))?;
        let path = self.path_for(key)?;
        self.paths
            .lock()
            .expect("replica paths lock poisoned")
            .insert(key.clone(), path.clone());
        version_token(&path)
    }
}

impl LocalReplica for LocalrefReplica {
    fn list_resources(
        &self,
        mut scopes: Vec<String>,
    ) -> Result<Vec<LocalResource>, SyncError> {
        if scopes.is_empty() {
            scopes = self
                .handle
                .block_on(self.client.list_items())
                .map_err(|e| error(e.to_string()))?
                .into_iter()
                .map(|item| item.id)
                .collect();
        }
        let mut output = Vec::new();
        for scope in scopes {
            let files = self
                .handle
                .block_on(self.client.item_files(&scope))
                .map_err(|e| error(e.to_string()))?;
            let root = self.library_root.join(&files.object_path);
            self.scope_roots
                .lock()
                .expect("scope roots lock poisoned")
                .insert(scope.clone(), root.clone());
            for entry in files.files.into_iter().filter(|entry| {
                entry.kind == "file" && entry.path != "metadata.toml"
            }) {
                safe_relative(&entry.path)?;
                let key = ResourceKey::new(&scope, &entry.path);
                let path = root.join(&entry.path);
                let token = version_token(&path)?;
                self.paths
                    .lock()
                    .expect("replica paths lock poisoned")
                    .insert(key.clone(), path);
                output.push(LocalResource {
                    key,
                    kind: ResourceKind::Binary,
                    state: ReplicaState::Present {
                        content_id: String::new(),
                        size: entry.bytes.unwrap_or(0),
                        version_token: token,
                    },
                });
            }
        }
        Ok(output)
    }

    fn read_resources(
        &self,
        keys: Vec<ResourceKey>,
    ) -> Result<Vec<ResourceContent>, SyncError> {
        // A batch may race with an editor or file deletion. Omit only the
        // unreadable resource; runtime turns the missing result into a
        // per-resource failure while continuing the rest of the scope.
        Ok(keys
            .into_iter()
            .filter_map(|key| {
                let path = self.path_for(&key).ok()?;
                let data = std::fs::read(&path).ok()?;
                let version_token = version_token(&path).ok()?;
                Some(ResourceContent {
                    key,
                    kind: ResourceKind::Binary,
                    version_token,
                    data,
                })
            })
            .collect())
    }

    fn apply_mutations(
        &self,
        mutations: Vec<LocalMutation>,
    ) -> Result<Vec<LocalApplyResult>, SyncError> {
        let mut output = Vec::with_capacity(mutations.len());
        for mutation in mutations {
            let key = match &mutation {
                LocalMutation::WritePresent { key, .. }
                | LocalMutation::ApplyDelete { key, .. }
                | LocalMutation::CreateCopy { key, .. } => key.clone(),
            };
            let result = match mutation {
                LocalMutation::WritePresent { data, precondition, .. } => {
                    let path = self.path_for(&key)?;
                    if !self.matches_precondition(&path, &precondition) {
                        LocalApplyResult {
                            key: key.clone(),
                            status: ApplyStatus::PreconditionFailed,
                            version_token: None,
                            error: None,
                        }
                    } else {
                        let token = if path.exists() {
                            std::fs::write(&path, data)
                                .map_err(|e| error(e.to_string()))?;
                            version_token(&path)?
                        } else {
                            self.add_at(&key, &data)?
                        };
                        LocalApplyResult {
                            key: key.clone(),
                            status: ApplyStatus::Applied,
                            version_token: Some(token),
                            error: None,
                        }
                    }
                }
                LocalMutation::ApplyDelete { precondition, .. } => {
                    let path = self.path_for(&key)?;
                    if !self.matches_precondition(&path, &precondition) {
                        LocalApplyResult {
                            key: key.clone(),
                            status: ApplyStatus::PreconditionFailed,
                            version_token: None,
                            error: None,
                        }
                    } else {
                        self.handle
                            .block_on(self.client.archive_file(
                                &key.scope_id,
                                &key.resource_id,
                                NS,
                            ))
                            .map_err(|e| error(e.to_string()))?;
                        self.paths
                            .lock()
                            .expect("replica paths lock poisoned")
                            .remove(&key);
                        LocalApplyResult {
                            key: key.clone(),
                            status: ApplyStatus::Applied,
                            version_token: None,
                            error: None,
                        }
                    }
                }
                LocalMutation::CreateCopy { data, .. } => {
                    let path = self.path_for(&key)?;
                    if path.exists() {
                        if std::fs::read(&path)
                            .is_ok_and(|existing| existing == data)
                        {
                            LocalApplyResult {
                                key: key.clone(),
                                status: ApplyStatus::Applied,
                                version_token: Some(version_token(&path)?),
                                error: None,
                            }
                        } else {
                            LocalApplyResult {
                                key: key.clone(),
                                status: ApplyStatus::PreconditionFailed,
                                version_token: None,
                                error: Some(
                                    "copy target already exists".into(),
                                ),
                            }
                        }
                    } else {
                        LocalApplyResult {
                            key: key.clone(),
                            status: ApplyStatus::Applied,
                            version_token: Some(self.add_at(&key, &data)?),
                            error: None,
                        }
                    }
                }
            };
            output.push(result);
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal() {
        assert!(safe_relative("../secret").is_err());
        assert!(safe_relative("/absolute").is_err());
        assert!(safe_relative("nested/file.pdf").is_ok());
    }
}
