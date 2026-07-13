//! Per-file sync baselines: the converged chunk manifest each local file
//! matched at the end of its last successful sync.
//!
//! The plugin runs as a fresh subprocess per invocation, so the sync engine's
//! in-memory state is empty at every run's start. To choose correctly between
//! *pushing* a local edit and *pulling* a remote that advanced, `sync_all`
//! needs a third reference point besides the current disk and the freshly
//! converged remote: what the file looked like when it was last in sync. That
//! is this baseline, persisted in the plugin state dir so it survives across
//! invocations.
//!
//! It is a rebuildable cache, not a source of truth: a missing or unreadable
//! baseline just makes the decision fall back to the conservative "push local"
//! (a divergence is then resolved by the engine's `KeepBoth` policy, which
//! never drops a local edit).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File-id → converged chunk manifest (ordered blake3 hashes) recorded at the
/// last successful sync of that file.
#[derive(Default, Serialize, Deserialize)]
pub struct Baselines {
    /// Keyed by engine `file_id` (`"{item_id}/{rel}"`).
    files: HashMap<String, Vec<String>>,
}

/// Baseline store path under the plugin state dir.
pub fn baselines_path(plugin_dir: &Path) -> PathBuf {
    plugin_dir.join("baselines.json")
}

impl Baselines {
    /// Load the store, treating any missing/corrupt file as empty — it is a
    /// rebuildable cache, so a read failure must not fail the sync.
    pub fn load(plugin_dir: &Path) -> Self {
        match std::fs::read(baselines_path(plugin_dir)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist atomically (temp file + rename) so a crash mid-write can't leave
    /// a truncated store that would poison the next run's decisions.
    pub fn save(&self, plugin_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(plugin_dir).map_err(|e| e.to_string())?;
        let path = baselines_path(plugin_dir);
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// The recorded baseline manifest for a file, if any.
    pub fn get(&self, file_id: &str) -> Option<&Vec<String>> {
        self.files.get(file_id)
    }

    /// Record `manifest` as the file's new baseline (its in-sync state).
    pub fn set(&mut self, file_id: String, manifest: Vec<String>) {
        self.files.insert(file_id, manifest);
    }
}
