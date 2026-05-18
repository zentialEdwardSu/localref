//! Filesystem lock files for daemon writes.
//!
//! Locks are represented by files created with `create_new`, which makes lock
//! acquisition atomic for local filesystems. The current milestone uses these
//! locks to serialize daemon write commands and surface conflicts instead of
//! silently overwriting external edits.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use error::{LocalrefError, Result};

/// Manager for lock files under `.localref/locks`.
#[derive(Clone, Debug)]
pub struct LockManager {
    dir: PathBuf,
}

/// Acquired filesystem lock removed when dropped.
#[derive(Debug)]
pub struct FsLock {
    path: PathBuf,
}

impl LockManager {
    /// Create a lock manager for one library root.
    pub fn new(library_root: impl Into<PathBuf>) -> Self {
        Self { dir: library_root.into().join(".localref").join("locks") }
    }

    /// Acquire an exclusive lock for one write key.
    pub fn acquire(
        &self,
        key: impl AsRef<str>,
        operation: impl AsRef<str>,
    ) -> Result<FsLock> {
        fs::create_dir_all(&self.dir)
            .map_err(|source| LocalrefError::io(&self.dir, source))?;
        let filename = format!("{}.lock", lock_component(key.as_ref()));
        let path = self.dir.join(filename);
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|source| {
                if source.kind() == std::io::ErrorKind::AlreadyExists {
                    LocalrefError::Conflict(format!(
                        "lock already exists for {}",
                        key.as_ref()
                    ))
                } else {
                    LocalrefError::io(&path, source)
                }
            })?;
        writeln!(file, "owner = \"daemon\"")
            .map_err(|source| LocalrefError::io(&path, source))?;
        writeln!(file, "operation = \"{}\"", operation.as_ref())
            .map_err(|source| LocalrefError::io(&path, source))?;
        file.sync_all().map_err(|source| LocalrefError::io(&path, source))?;
        Ok(FsLock { path })
    }
}

impl Drop for FsLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => output.push(ch),
            _ => output.push('_'),
        }
    }
    if output.is_empty() { "lock".to_string() } else { output }
}
