//! File listing and system-open helpers for REST item file endpoints.
//!
//! The helpers keep path validation close to the endpoints that expose local
//! files. They only accept paths relative to an indexed item directory.

use std::path::{Component, Path, PathBuf};
#[cfg(not(windows))]
use std::process::Command;

use crate::LocalrefDaemon;
use crate::error::{LocalrefError, Result};
use crate::model::{ItemFileEntry, ItemFilesDocument};

/// Return all filesystem entries currently under one indexed item directory.
pub fn item_files(
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
pub fn item_folder(
    daemon: &LocalrefDaemon,
    item_id: &str,
) -> Result<Option<PathBuf>> {
    let Some(item) = daemon.get_item(item_id)? else {
        return Ok(None);
    };
    Ok(Some(daemon.library_root.join(item.object_path)))
}

/// Resolve a validated item-relative file path to an absolute path.
pub fn item_file_path(
    daemon: &LocalrefDaemon,
    item_id: &str,
    relative: &Path,
) -> Result<Option<PathBuf>> {
    if !is_item_relative_path(relative) {
        return Err(LocalrefError::Unsupported("invalid item file path"));
    }
    let Some(folder) = item_folder(daemon, item_id)? else {
        return Ok(None);
    };
    let path = folder.join(relative);
    if path.exists() { Ok(Some(path)) } else { Ok(None) }
}

/// Open a file or directory with the platform's default viewer.
#[cfg(windows)]
pub fn open_system_path(path: &Path) -> Result<()> {
    native_win32::open_path(path)
        .map_err(|source| LocalrefError::Platform(source.to_string()))
}

/// Open a file or directory with the platform's default viewer.
#[cfg(not(windows))]
pub fn open_system_path(path: &Path) -> Result<()> {
    let mut command = system_open_command(path);
    command.spawn().map_err(|source| LocalrefError::io(path, source))?;
    Ok(())
}

/// Return whether a user supplied path stays inside an item directory.
pub fn is_item_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

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

#[cfg(target_os = "macos")]
fn system_open_command(path: &Path) -> Command {
    let mut command = Command::new("open");
    command.arg(path);
    command
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_open_command(path: &Path) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(path);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn item_relative_paths_reject_escape_attempts() {
        assert!(is_item_relative_path(Path::new("paper.pdf")));
        assert!(is_item_relative_path(Path::new("figures/one.png")));
        assert!(!is_item_relative_path(Path::new("../paper.pdf")));
        assert!(!is_item_relative_path(Path::new("/tmp/paper.pdf")));
        assert!(!is_item_relative_path(Path::new("")));
    }
}
