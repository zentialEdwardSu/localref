//! Read-only filesystem scanner for Localref libraries.
//!
//! The scanner discovers facts from `All/` and `Cat/` without mutating the
//! library. Core can later turn these facts into queue tasks, warnings, events,
//! or normalization operations.

use std::fs;
use std::path::Path;

use error::{LocalrefError, Result};
use serde::{Deserialize, Serialize};
use types::CategoryPath;

/// Complete read-only scan result for one library root.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct LibraryScan {
    /// Entries discovered directly under `All/`.
    pub all_entries: Vec<AllEntry>,
    /// Entries discovered under `Cat/`.
    pub cat_entries: Vec<CatEntry>,
}

/// One first-level entry under `All/`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct AllEntry {
    /// Library-relative path.
    pub path: String,
    /// Entry state.
    pub kind: AllEntryKind,
}

/// State of one `All/` entry.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AllEntryKind {
    /// Directory containing `metadata.toml`.
    ManagedItem,
    /// Directory containing `.localrefignore`.
    Ignored,
    /// Directory without metadata that can become a manual import candidate.
    UnmanagedCandidate,
    /// File directly under `All/`, which is invalid for phase one.
    UnmanagedAllFile,
}

/// One entry discovered under `Cat/`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct CatEntry {
    /// Category path relative to `Cat/`.
    pub category: Option<CategoryPath>,
    /// Library-relative path of the Cat entry.
    pub path: String,
    /// Entry kind.
    pub kind: CatEntryKind,
    /// Target path for links, when resolvable.
    pub target_path: Option<String>,
}

/// State of one `Cat/` entry.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatEntryKind {
    /// Ordinary category directory.
    CategoryDirectory,
    /// Directory link or junction pointing at `All/<item>`.
    ItemLink,
    /// Directory link whose target is invalid or outside `All/`.
    BrokenLink,
    /// Real directory under `Cat/` that should be normalized later.
    RealDirectoryCandidate,
    /// File under `Cat/`, which is invalid for phase one.
    InvalidFile,
}

/// Scan `All/` and `Cat/` under one library root.
pub fn scan_library(library_root: impl AsRef<Path>) -> Result<LibraryScan> {
    let root = library_root.as_ref();
    Ok(LibraryScan {
        all_entries: scan_all(root)?,
        cat_entries: scan_cat(root)?,
    })
}

/// Scan direct children of `All/`.
pub fn scan_all(library_root: impl AsRef<Path>) -> Result<Vec<AllEntry>> {
    let root = library_root.as_ref();
    let all_dir = root.join("All");
    if !all_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&all_dir)
        .map_err(|source| LocalrefError::io(&all_dir, source))?
    {
        let entry =
            entry.map_err(|source| LocalrefError::io(&all_dir, source))?;
        let path = entry.path();
        let kind = if path.is_dir() {
            if path.join(".localrefignore").exists() {
                AllEntryKind::Ignored
            } else if path.join("metadata.toml").exists() {
                AllEntryKind::ManagedItem
            } else {
                AllEntryKind::UnmanagedCandidate
            }
        } else {
            AllEntryKind::UnmanagedAllFile
        };
        entries.push(AllEntry { path: relative(root, &path), kind });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

/// Scan `Cat/` recursively without following item links.
pub fn scan_cat(library_root: impl AsRef<Path>) -> Result<Vec<CatEntry>> {
    let root = library_root.as_ref();
    let cat_dir = root.join("Cat");
    if !cat_dir.exists() {
        return Ok(Vec::new());
    }
    let all_dir = root.join("All");
    let all_canonical = all_dir.canonicalize().ok();
    let mut entries = Vec::new();
    scan_cat_dir(
        root,
        &cat_dir,
        &cat_dir,
        all_canonical.as_deref(),
        &mut entries,
    )?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn scan_cat_dir(
    root: &Path,
    cat_root: &Path,
    dir: &Path,
    all_canonical: Option<&Path>,
    entries: &mut Vec<CatEntry>,
) -> Result<()> {
    for entry in
        fs::read_dir(dir).map_err(|source| LocalrefError::io(dir, source))?
    {
        let entry = entry.map_err(|source| LocalrefError::io(dir, source))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| LocalrefError::io(&path, source))?;
        let rel_cat = path.strip_prefix(cat_root).unwrap_or(&path);
        let category = category_for(rel_cat);

        if metadata.file_type().is_file() {
            entries.push(CatEntry {
                category,
                path: relative(root, &path),
                kind: CatEntryKind::InvalidFile,
                target_path: None,
            });
            continue;
        }

        if metadata.file_type().is_symlink() {
            let target = path.canonicalize().ok();
            let valid = target
                .as_deref()
                .zip(all_canonical)
                .is_some_and(|(target, all)| target.starts_with(all));
            entries.push(CatEntry {
                category,
                path: relative(root, &path),
                kind: if valid {
                    CatEntryKind::ItemLink
                } else {
                    CatEntryKind::BrokenLink
                },
                target_path: target
                    .as_deref()
                    .map(|target| relative(root, target)),
            });
            continue;
        }

        if path.is_dir() {
            let target = path.canonicalize().ok();
            if target
                .as_deref()
                .zip(all_canonical)
                .is_some_and(|(target, all)| target.starts_with(all))
            {
                entries.push(CatEntry {
                    category,
                    path: relative(root, &path),
                    kind: CatEntryKind::ItemLink,
                    target_path: target
                        .as_deref()
                        .map(|target| relative(root, target)),
                });
            } else if category.is_some()
                && looks_like_cat_item_directory(&path)?
            {
                entries.push(CatEntry {
                    category,
                    path: relative(root, &path),
                    kind: CatEntryKind::RealDirectoryCandidate,
                    target_path: None,
                });
            } else {
                entries.push(CatEntry {
                    category: category.clone(),
                    path: relative(root, &path),
                    kind: CatEntryKind::CategoryDirectory,
                    target_path: None,
                });
                scan_cat_dir(root, cat_root, &path, all_canonical, entries)?;
            }
        }
    }
    Ok(())
}

/// Return true when a real `Cat/` directory looks like a literature item.
fn looks_like_cat_item_directory(path: &Path) -> Result<bool> {
    if path.join("metadata.toml").exists() {
        return Ok(true);
    }
    for entry in
        fs::read_dir(path).map_err(|source| LocalrefError::io(path, source))?
    {
        let entry = entry.map_err(|source| LocalrefError::io(path, source))?;
        if entry
            .file_type()
            .map_err(|source| LocalrefError::io(entry.path(), source))?
            .is_file()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn category_for(rel_cat_entry: &Path) -> Option<CategoryPath> {
    let parent = rel_cat_entry.parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    CategoryPath::new(parent.to_string_lossy().replace('\\', "/"))
}

fn relative(root: &Path, path: &Path) -> String {
    let normalized = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if normalized.len() >= 2 && normalized.as_bytes()[1] == b':' {
        path.to_string_lossy().replace('\\', "/")
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_all_managed_unmanaged_ignored_and_files() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("All").join("Managed")).unwrap();
        fs::write(
            temp.path().join("All").join("Managed").join("metadata.toml"),
            "id = 'x'",
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("All").join("Unmanaged")).unwrap();
        fs::create_dir_all(temp.path().join("All").join("Ignored")).unwrap();
        fs::write(
            temp.path().join("All").join("Ignored").join(".localrefignore"),
            "",
        )
        .unwrap();
        fs::write(temp.path().join("All").join("loose.pdf"), "").unwrap();

        let entries = scan_all(temp.path()).unwrap();

        assert_eq!(entries.len(), 4);
        assert!(
            entries
                .iter()
                .any(|entry| entry.kind == AllEntryKind::ManagedItem)
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.kind == AllEntryKind::UnmanagedCandidate)
        );
        assert!(
            entries.iter().any(|entry| entry.kind == AllEntryKind::Ignored)
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.kind == AllEntryKind::UnmanagedAllFile)
        );
    }

    #[test]
    fn scans_cat_category_link_and_invalid_file() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Paper One");
        fs::create_dir_all(&item_dir).unwrap();
        fs::write(item_dir.join("metadata.toml"), "id = 'x'").unwrap();
        let category_dir = temp.path().join("Cat").join("Wireless");
        fs::create_dir_all(&category_dir).unwrap();
        fs::write(category_dir.join("loose.pdf"), "").unwrap();

        platformfs::LibraryFs::new(temp.path())
            .create_category_link(
                &CategoryPath::new("Wireless").unwrap(),
                &item_dir,
            )
            .unwrap();

        let entries = scan_cat(temp.path()).unwrap();

        assert!(entries.iter().any(|entry| {
            entry.kind == CatEntryKind::ItemLink
                && entry
                    .category
                    .as_ref()
                    .is_some_and(|category| category.as_str() == "Wireless")
        }));
        assert!(
            entries
                .iter()
                .any(|entry| entry.kind == CatEntryKind::InvalidFile)
        );
    }

    #[test]
    fn scans_real_cat_directory_with_plain_file_as_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let paper_dir = temp.path().join("Cat").join("Inbox").join("Copied");
        fs::create_dir_all(&paper_dir).unwrap();
        fs::write(paper_dir.join("paper.pdf"), "pdf").unwrap();

        let entries = scan_cat(temp.path()).unwrap();

        assert!(entries.iter().any(|entry| {
            entry.kind == CatEntryKind::RealDirectoryCandidate
                && entry.path == "Cat/Inbox/Copied"
                && entry
                    .category
                    .as_ref()
                    .is_some_and(|category| category.as_str() == "Inbox")
        }));
    }
}
