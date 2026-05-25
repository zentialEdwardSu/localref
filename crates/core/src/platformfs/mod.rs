//! NTFS-aware filesystem operations for Localref.
//!
//! This crate owns side-effecting filesystem operations. The current slice
//! focuses on creating the library layout, creating unique `All/<item>/`
//! directories, and atomically writing files into those directories.
//!
//! On Windows/NTFS the implementation avoids invalid path components, reserved
//! DOS device names, and trailing spaces or dots. Atomic writes use a temporary
//! sibling file followed by `rename`, which maps to an atomic replace operation
//! on NTFS for files in the same directory. The public language in this crate
//! uses "flush" semantics for durability barriers; the Rust primitive underneath
//! is `File::sync_all()`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(not(windows))]
use std::fs::File;

use crate::error::{LocalrefError, Result};
use crate::types::CategoryPath;

/// Filesystem helper rooted at one Localref library.
#[derive(Clone, Debug)]
pub struct LibraryFs {
    root: PathBuf,
}

impl LibraryFs {
    /// Create a filesystem helper for a library root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Return the library root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the `All/` directory path.
    pub fn all_dir(&self) -> PathBuf {
        self.root.join("All")
    }

    /// Return the `Cat/` directory path.
    pub fn cat_dir(&self) -> PathBuf {
        self.root.join("Cat")
    }

    /// Return the `.localref/` directory path.
    pub fn state_dir(&self) -> PathBuf {
        self.root.join(".localref")
    }

    /// Ensure the stage-one Localref directory layout exists.
    pub fn ensure_layout(&self) -> Result<()> {
        for path in [
            self.all_dir(),
            self.cat_dir(),
            self.state_dir(),
            self.state_dir().join("staging"),
            self.state_dir().join("locks"),
            self.state_dir().join("logs"),
        ] {
            fs::create_dir_all(&path)
                .map_err(|source| LocalrefError::io(path, source))?;
        }
        Ok(())
    }

    /// Create a unique item directory under `All/`.
    pub fn create_unique_item_dir(&self, title: &str) -> Result<PathBuf> {
        let base = sanitize_ntfs_component(title)?;
        let mut candidate = self.all_dir().join(&base);
        let mut suffix = 2_u32;

        while candidate.exists() {
            candidate = self.all_dir().join(format!("{base} ({suffix})"));
            suffix += 1;
        }

        fs::create_dir_all(&candidate)
            .map_err(|source| LocalrefError::io(candidate.clone(), source))?;
        Ok(candidate)
    }

    /// Atomically write bytes to a path and flush the temporary file before rename.
    pub fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|source| LocalrefError::io(parent, source))?;
        }

        let tmp = temporary_sibling(path)?;
        {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp)
                .map_err(|source| LocalrefError::io(tmp.clone(), source))?;
            file.write_all(bytes)
                .map_err(|source| LocalrefError::io(tmp.clone(), source))?;
            file.sync_all()
                .map_err(|source| LocalrefError::io(tmp.clone(), source))?;
        }

        fs::rename(&tmp, path)
            .map_err(|source| LocalrefError::io(path, source))?;
        flush_parent_dir(path)?;
        Ok(())
    }

    /// Create a category directory link under `Cat/` pointing at an `All/` item.
    pub fn create_category_link(
        &self,
        category: &CategoryPath,
        item_dir: &Path,
    ) -> Result<PathBuf> {
        self.ensure_target_is_all_item(item_dir)?;
        let category_dir = self.category_dir(category)?;
        fs::create_dir_all(&category_dir)
            .map_err(|source| LocalrefError::io(&category_dir, source))?;
        let item_name = item_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| LocalrefError::InvalidPathComponent {
                component: item_dir.display().to_string(),
                reason: "item directory has no UTF-8 file name",
            })?;
        self.create_category_link_named(category, item_name, item_dir)
    }

    /// Create an empty category directory under `Cat/`.
    pub fn create_category_dir(
        &self,
        category: &CategoryPath,
    ) -> Result<PathBuf> {
        let category_dir = self.category_dir(category)?;
        fs::create_dir_all(&category_dir)
            .map_err(|source| LocalrefError::io(&category_dir, source))?;
        Ok(category_dir)
    }

    /// Create a category link using an explicit entry name.
    pub fn create_category_link_named(
        &self,
        category: &CategoryPath,
        entry_name: &str,
        item_dir: &Path,
    ) -> Result<PathBuf> {
        self.ensure_target_is_all_item(item_dir)?;
        let category_dir = self.category_dir(category)?;
        fs::create_dir_all(&category_dir)
            .map_err(|source| LocalrefError::io(&category_dir, source))?;
        let link_name = sanitize_ntfs_component(entry_name)?;
        let link_path = category_dir.join(link_name);
        if link_path.exists() {
            return Ok(link_path);
        }
        create_dir_link(item_dir, &link_path)?;
        Ok(link_path)
    }

    /// Remove a category link while leaving the `All/` target untouched.
    pub fn remove_category_link(
        &self,
        category: &CategoryPath,
        entry_name: &str,
    ) -> Result<Option<PathBuf>> {
        let link_path = self
            .category_dir(category)?
            .join(sanitize_ntfs_component(entry_name)?);
        if !link_path.exists() {
            return Ok(None);
        }
        self.ensure_target_is_all_item(&link_path)?;
        fs::remove_dir(&link_path)
            .map_err(|source| LocalrefError::io(&link_path, source))?;
        Ok(Some(link_path))
    }

    /// Rename a category directory.
    pub fn rename_category(
        &self,
        from: &CategoryPath,
        to: &CategoryPath,
    ) -> Result<PathBuf> {
        let from_path = self.category_dir(from)?;
        let to_path = self.category_dir(to)?;
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|source| LocalrefError::io(parent, source))?;
        }
        fs::rename(&from_path, &to_path)
            .map_err(|source| LocalrefError::io(&to_path, source))?;
        Ok(to_path)
    }

    /// Move all entries from one category into another and remove the source.
    pub fn merge_category(
        &self,
        from: &CategoryPath,
        to: &CategoryPath,
    ) -> Result<PathBuf> {
        let from_path = self.category_dir(from)?;
        let to_path = self.category_dir(to)?;
        fs::create_dir_all(&to_path)
            .map_err(|source| LocalrefError::io(&to_path, source))?;
        if !from_path.exists() {
            return Ok(to_path);
        }
        for entry in fs::read_dir(&from_path)
            .map_err(|source| LocalrefError::io(&from_path, source))?
        {
            let entry = entry
                .map_err(|source| LocalrefError::io(&from_path, source))?;
            let target = to_path.join(entry.file_name());
            if target.exists() {
                continue;
            }
            fs::rename(entry.path(), &target)
                .map_err(|source| LocalrefError::io(&target, source))?;
        }
        fs::remove_dir(&from_path)
            .map_err(|source| LocalrefError::io(&from_path, source))?;
        Ok(to_path)
    }

    /// Return the filesystem path for a category.
    pub fn category_dir(&self, category: &CategoryPath) -> Result<PathBuf> {
        let mut path = self.cat_dir();
        for component in category.components() {
            path.push(sanitize_ntfs_component(component)?);
        }
        Ok(path)
    }

    fn ensure_target_is_all_item(&self, item_dir: &Path) -> Result<()> {
        let all_dir = self
            .all_dir()
            .canonicalize()
            .map_err(|source| LocalrefError::io(self.all_dir(), source))?;
        let target = item_dir
            .canonicalize()
            .map_err(|source| LocalrefError::io(item_dir, source))?;
        if !target.starts_with(&all_dir) || target == all_dir {
            return Err(LocalrefError::InvalidPathComponent {
                component: item_dir.display().to_string(),
                reason: "category links must target an All/ item directory",
            });
        }
        Ok(())
    }
}

/// Sanitize one filename/path component according to NTFS constraints.
pub fn sanitize_ntfs_component(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(LocalrefError::MissingField("path component"));
    }

    let mut sanitized = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => {
                sanitized.push('_')
            }
            '\u{0}'..='\u{1f}' => sanitized.push('_'),
            _ => sanitized.push(ch),
        }
    }

    while sanitized.ends_with([' ', '.']) {
        sanitized.pop();
    }

    if sanitized.is_empty() {
        return Err(LocalrefError::InvalidPathComponent {
            component: value.to_string(),
            reason: "component becomes empty after NTFS sanitization",
        });
    }

    if is_reserved_windows_name(&sanitized) {
        sanitized.push('_');
    }

    Ok(sanitized)
}

fn temporary_sibling(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| LocalrefError::InvalidPathComponent {
            component: path.display().to_string(),
            reason: "target path has no UTF-8 file name",
        })?;
    let tmp_name = format!(".{filename}.localref-tmp");
    Ok(path.with_file_name(tmp_name))
}

fn flush_parent_dir(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };

    #[cfg(windows)]
    {
        // NTFS gives the atomicity guarantee needed here through the
        // same-directory rename after the temporary file has been flushed.
        // Opening a directory handle for `FlushFileBuffers` is not reliably
        // available to normal desktop processes, so Windows does not fail the
        // import when parent-directory flushing is unavailable.
        let _ = parent;
    }

    #[cfg(not(windows))]
    {
        let file = File::open(parent)
            .map_err(|source| LocalrefError::io(parent, source))?;
        file.sync_all().map_err(|source| LocalrefError::io(parent, source))?;
    }

    Ok(())
}

fn is_reserved_windows_name(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or(value).to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

#[cfg(windows)]
fn create_dir_link(target: &Path, link: &Path) -> Result<()> {
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .map_err(|source| LocalrefError::io(link, source))?;
    if output.status.success() {
        return Ok(());
    }

    std::os::windows::fs::symlink_dir(target, link).map_err(|source| {
        LocalrefError::io(
            link,
            std::io::Error::new(
                source.kind(),
                format!(
                    "failed to create junction or directory symlink: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            ),
        )
    })
}

#[cfg(unix)]
fn create_dir_link(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|source| LocalrefError::io(link, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_ntfs_components() {
        assert_eq!(sanitize_ntfs_component("A:B?C. ").unwrap(), "A_B_C");
        assert_eq!(sanitize_ntfs_component("CON").unwrap(), "CON_");
    }

    #[test]
    fn creates_layout_and_writes_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let fs = LibraryFs::new(temp.path());

        fs.ensure_layout().unwrap();
        let item_dir = fs.create_unique_item_dir("Paper: One").unwrap();
        let target = item_dir.join("paper.pdf");
        fs.atomic_write(&target, b"pdf").unwrap();

        assert!(fs.all_dir().exists());
        assert!(fs.cat_dir().exists());
        assert!(fs.state_dir().join("staging").exists());
        assert_eq!(std::fs::read(target).unwrap(), b"pdf");
    }

    #[test]
    fn creates_category_directory_link() {
        let temp = tempfile::tempdir().unwrap();
        let fs = LibraryFs::new(temp.path());
        fs.ensure_layout().unwrap();
        let item_dir = fs.create_unique_item_dir("Paper One").unwrap();
        let category = CategoryPath::new("Wireless/RIS").unwrap();

        let link = fs.create_category_link(&category, &item_dir).unwrap();

        assert!(link.exists());
    }

    #[test]
    fn creates_empty_category_directory() {
        let temp = tempfile::tempdir().unwrap();
        let fs = LibraryFs::new(temp.path());
        fs.ensure_layout().unwrap();

        let path = fs
            .create_category_dir(&CategoryPath::new("Wireless/RIS").unwrap())
            .unwrap();

        assert!(path.ends_with("Cat/Wireless/RIS"));
        assert!(path.is_dir());
    }
}
