//! Durable manual-conflict and retry-queue state for s3sync.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A single file awaiting an explicit user choice.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConflictRecord {
    /// Stable selection key used by the plugin UI.
    pub id: String,
    pub item_id: String,
    pub file_id: String,
    pub relative_path: String,
    pub detected_at_ms: i64,
    pub local_manifest: Vec<String>,
    pub remote_manifest: Vec<String>,
}

/// State that must survive each fresh plugin process invocation.
#[derive(Default, Serialize, Deserialize)]
pub struct SyncState {
    #[serde(default)]
    conflicts: HashMap<String, ConflictRecord>,
    #[serde(default)]
    pending_items: BTreeSet<String>,
}

fn path(plugin_dir: &Path) -> PathBuf {
    plugin_dir.join("sync-state.json")
}

impl SyncState {
    pub fn load(plugin_dir: &Path) -> Self {
        std::fs::read(path(plugin_dir))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, plugin_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(plugin_dir).map_err(|e| e.to_string())?;
        let destination = path(plugin_dir);
        let temporary = destination.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(temporary, destination).map_err(|e| e.to_string())
    }

    pub fn blocks_item(&self, item_id: &str) -> bool {
        self.conflicts.values().any(|record| record.item_id == item_id)
    }

    pub fn record(&mut self, record: ConflictRecord) {
        self.conflicts.insert(record.id.clone(), record);
    }

    pub fn get(&self, id: &str) -> Option<&ConflictRecord> {
        self.conflicts.get(id)
    }

    pub fn resolve(&mut self, id: &str) -> Option<ConflictRecord> {
        let record = self.conflicts.remove(id)?;
        self.pending_items.insert(record.item_id.clone());
        Some(record)
    }

    pub fn pending_first(&self, requested: &[String]) -> Vec<String> {
        let mut ordered: Vec<String> = self.pending_items.iter().cloned().collect();
        for item_id in requested {
            if !ordered.contains(item_id) {
                ordered.push(item_id.clone());
            }
        }
        ordered
    }

    pub fn complete_item(&mut self, item_id: &str) {
        self.pending_items.remove(item_id);
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &ConflictRecord> {
        self.conflicts.values()
    }
}

#[cfg(test)]
mod tests {
    use super::{ConflictRecord, SyncState};

    fn record() -> ConflictRecord {
        ConflictRecord {
            id: "item/file.pdf".to_owned(),
            item_id: "item".to_owned(),
            file_id: "item/file.pdf".to_owned(),
            relative_path: "file.pdf".to_owned(),
            detected_at_ms: 1,
            local_manifest: vec!["local".to_owned()],
            remote_manifest: vec!["remote".to_owned()],
        }
    }

    #[test]
    fn conflict_blocks_item_and_resolution_becomes_pending_across_reload() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = SyncState::default();
        state.record(record());
        assert!(state.blocks_item("item"));
        state.save(directory.path()).unwrap();

        let mut loaded = SyncState::load(directory.path());
        let resolved = loaded.resolve("item/file.pdf").unwrap();
        assert_eq!(resolved.relative_path, "file.pdf");
        assert_eq!(loaded.pending_first(&[]), vec!["item".to_owned()]);
        loaded.complete_item("item");
        assert!(loaded.pending_first(&[]).is_empty());
    }
}
