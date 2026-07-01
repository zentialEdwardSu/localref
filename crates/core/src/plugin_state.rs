//! Persisted plugin enable/disable state.
//!
//! Disabled plugin names are stored in `<library>/.localref/plugin-state.toml`.
//! A disabled plugin is filtered out of UI surfacing and skipped by the hook
//! dispatcher and cron scheduler, without removing it from disk. State persists
//! across daemon restarts.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::{LocalrefError, Result};
use serde::{Deserialize, Serialize};

/// On-disk wrapper for the plugin-state TOML file.
#[derive(Debug, Default, Deserialize, Serialize)]
struct PluginStateFile {
    /// Names of plugins the user has disabled.
    #[serde(default)]
    disabled: Vec<String>,
}

/// Return the plugin-state file path for a library root.
#[must_use]
pub fn plugin_state_path(library_root: &Path) -> PathBuf {
    library_root.join(".localref").join("plugin-state.toml")
}

/// Load the set of disabled plugin names for a library root.
///
/// A missing file is normal and yields an empty set.
///
/// # Errors
/// Returns an error when the file exists but cannot be read or parsed.
pub fn load_disabled(library_root: &Path) -> Result<BTreeSet<String>> {
    let path = plugin_state_path(library_root);
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|source| LocalrefError::io(&path, source))?;
    let file: PluginStateFile = toml::from_str(&text)?;
    Ok(file.disabled.into_iter().collect())
}

/// Persist the set of disabled plugin names for a library root.
///
/// # Errors
/// Returns an error when the directory or file cannot be written, or when the
/// set cannot be serialized.
pub fn save_disabled(
    library_root: &Path,
    disabled: &BTreeSet<String>,
) -> Result<()> {
    let path = plugin_state_path(library_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| LocalrefError::io(parent, source))?;
    }
    let file =
        PluginStateFile { disabled: disabled.iter().cloned().collect() };
    let text = toml::to_string_pretty(&file)?;
    std::fs::write(&path, text)
        .map_err(|source| LocalrefError::io(&path, source))
}

#[cfg(test)]
mod tests {
    use super::{load_disabled, save_disabled};
    use std::collections::BTreeSet;

    #[test]
    fn load_missing_file_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load_disabled(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let mut disabled = BTreeSet::new();
        let _ = disabled.insert("archiver".to_string());
        let _ = disabled.insert("hooklog".to_string());
        save_disabled(temp.path(), &disabled).unwrap();

        assert_eq!(load_disabled(temp.path()).unwrap(), disabled);
    }
}
