//! Runtime-registered scheduled plugin calls.
//!
//! Schedules are persisted to `<library>/.localref/schedules.toml`. Unlike the
//! static `[[cron]]` jobs declared in a plugin's `plugin.toml` (which only
//! invoke the declaring plugin), a [`ScheduledCall`] names a target plugin and
//! action, so a plugin can schedule itself or any other discovered plugin. The
//! daemon's cron scheduler loads these alongside manifest cron jobs.

use std::path::{Path, PathBuf};

use crate::error::{LocalrefError, Result};
pub use crate::model::ScheduledCall;
use serde::{Deserialize, Serialize};

/// On-disk wrapper for the schedules TOML file.
#[derive(Debug, Default, Deserialize, Serialize)]
struct SchedulesFile {
    /// Registered scheduled calls.
    #[serde(default)]
    schedule: Vec<ScheduledCall>,
}

/// Return the schedules file path for a library root.
#[must_use]
pub fn schedules_path(library_root: &Path) -> PathBuf {
    library_root.join(".localref").join("schedules.toml")
}

/// Load all registered schedules for a library root.
///
/// A missing file is normal and yields an empty list.
///
/// # Errors
/// Returns an error when the file exists but cannot be read or parsed.
pub fn load(library_root: &Path) -> Result<Vec<ScheduledCall>> {
    let path = schedules_path(library_root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|source| LocalrefError::io(&path, source))?;
    let file: SchedulesFile = toml::from_str(&text)?;
    Ok(file.schedule)
}

/// Persist the full set of schedules for a library root, creating parent dirs.
///
/// # Errors
/// Returns an error when the directory or file cannot be written, or when the
/// schedules cannot be serialized.
pub fn save(library_root: &Path, schedules: &[ScheduledCall]) -> Result<()> {
    let path = schedules_path(library_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| LocalrefError::io(parent, source))?;
    }
    let file = SchedulesFile { schedule: schedules.to_vec() };
    let text = toml::to_string_pretty(&file)?;
    std::fs::write(&path, text)
        .map_err(|source| LocalrefError::io(&path, source))
}

#[cfg(test)]
mod tests {
    use super::{load, save};
    use crate::model::ScheduledCall;
    use std::collections::BTreeMap;

    fn sample(id: &str) -> ScheduledCall {
        let mut params = BTreeMap::new();
        let _ = params.insert("format".to_string(), "bibtex".to_string());
        ScheduledCall {
            id: id.to_string(),
            plugin: "archiver".to_string(),
            action: "backup".to_string(),
            params,
            schedule: "0 0 3 * * *".to_string(),
        }
    }

    #[test]
    fn load_missing_file_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let schedules = vec![sample("nightly"), sample("weekly")];
        save(temp.path(), &schedules).unwrap();

        let loaded = load(temp.path()).unwrap();
        assert_eq!(loaded, schedules);
        // Params survive the round-trip.
        assert_eq!(loaded[0].params.get("format").map(String::as_str), Some("bibtex"));
    }
}
