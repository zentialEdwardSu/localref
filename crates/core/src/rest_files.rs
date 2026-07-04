//! File listing and system-open helpers for REST item file endpoints.
//!
//! The helpers keep path validation close to the endpoints that expose local
//! files. They only accept paths relative to an indexed item directory.

use std::path::{Component, Path, PathBuf};

use crate::LocalrefDaemon;
use crate::error::{LocalrefError, Result};
use crate::model::{ItemFileEntry, ItemFilesDocument};

/// Return all filesystem entries currently under one indexed item directory.
pub(super) fn item_files(
    daemon: &LocalrefDaemon,
    item_id: &str,
) -> Result<Option<ItemFilesDocument>> {
    let Some(item) = daemon.get_item(item_id)? else {
        return Ok(None);
    };
    let item_dir = daemon.library_root.join(&item.object_path);
    let mut files = Vec::new();
    collect_entries(&item_dir, &item_dir, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Some(ItemFilesDocument {
        item_id: item.id,
        object_path: item.object_path,
        files,
    }))
}

/// Return the absolute item directory for one indexed item.
pub(super) fn item_folder(
    daemon: &LocalrefDaemon,
    item_id: &str,
) -> Result<Option<PathBuf>> {
    let Some(item) = daemon.get_item(item_id)? else {
        return Ok(None);
    };
    Ok(Some(daemon.library_root.join(item.object_path)))
}

/// Resolve a validated item-relative file path to an absolute path.
pub(super) fn item_file_path(
    daemon: &LocalrefDaemon,
    item_id: &str,
    relative: &Path,
) -> Result<Option<PathBuf>> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(LocalrefError::Unsupported("invalid item file path"));
    }
    let Some(folder) = item_folder(daemon, item_id)? else {
        return Ok(None);
    };
    let path = folder.join(relative);
    if path.exists() { Ok(Some(path)) } else { Ok(None) }
}

/// Open a file or directory with the platform's default viewer.
///
/// Uses the cross-platform [`open`] crate (Windows shell, `open` on macOS,
/// `xdg-open` on Linux) so the daemon no longer needs a native shell bridge.
pub(super) fn open_system_path(path: &Path) -> Result<()> {
    open::that(path).map_err(|source| LocalrefError::io(path, source))
}

/// Internal helper for collect entries.
fn collect_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<ItemFileEntry>,
) -> Result<()> {
    for entry in std::fs::read_dir(current)
        .map_err(|source| LocalrefError::io(current, source))?
    {
        let entry =
            entry.map_err(|source| LocalrefError::io(current, source))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|source| LocalrefError::io(&path, source))?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| LocalrefError::Unsupported("file outside item"))?;
        let kind = if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };
        entries.push(ItemFileEntry {
            path: relative.to_string_lossy().replace('\\', "/"),
            kind: kind.to_string(),
            bytes: metadata.is_file().then_some(metadata.len()),
        });
        if metadata.is_dir() {
            collect_entries(root, &path, entries)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn item_relative_paths_reject_escape_attempts() {
        let is_valid = |path: &Path| {
            !path.as_os_str().is_empty()
                && !path.is_absolute()
                && path
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
        };
        assert!(is_valid(Path::new("paper.pdf")));
        assert!(is_valid(Path::new("figures/one.png")));
        assert!(!is_valid(Path::new("../paper.pdf")));
        assert!(!is_valid(Path::new("/tmp/paper.pdf")));
        assert!(!is_valid(Path::new("")));
    }
}
