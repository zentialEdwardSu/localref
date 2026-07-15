//! Daemon-projected category junction manifest.
//!
//! `metadata.toml` is the source of truth for category membership; the `Cat/`
//! junctions are a projection of it. This manifest records, per item id, the set
//! of categories for which the daemon has *projected a junction*. It is the diff
//! baseline that lets a scan classify each on-disk junction:
//!
//! - in metadata, on disk → in sync
//! - in metadata, junction gone → the junction is (re)projected from metadata
//!   (a deleted junction is not a removal; removal is metadata/UI/API only)
//! - on disk + manifest, not in metadata → the daemon projected it before and
//!   the user removed the category from metadata → remove the junction
//! - on disk, not in metadata nor manifest → a hand-made junction to adopt
//!
//! The manifest is never authoritative on its own — it exists only to make that
//! classification possible. It is persisted to
//! `<library>/.localref/cat-manifest.toml` and is safely rebuildable.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::{LocalrefError, Result};

/// Per-item set of categories for which a junction has been projected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatManifest {
    /// item id → set of projected category paths.
    entries: BTreeMap<String, BTreeSet<String>>,
}

/// Return the manifest file path for a library root.
#[must_use]
pub fn manifest_path(library_root: &Path) -> PathBuf {
    library_root.join(".localref").join("cat-manifest.toml")
}

impl CatManifest {
    /// Load the manifest for a library root; a missing file yields an empty one.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed.
    pub fn load(library_root: &Path) -> Result<Self> {
        let path = manifest_path(library_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|source| LocalrefError::io(&path, source))?;
        let doc: BTreeMap<String, Vec<String>> = toml::from_str(&text)?;
        let entries = doc
            .into_iter()
            .map(|(id, categories)| (id, categories.into_iter().collect()))
            .collect();
        Ok(Self { entries })
    }

    /// Persist the manifest for a library root, creating parent dirs.
    ///
    /// The document is built with `toml_edit` for consistency with the project's
    /// TOML-writing convention.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or file cannot be written.
    pub fn save(&self, library_root: &Path) -> Result<()> {
        let path = manifest_path(library_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| LocalrefError::io(parent, source))?;
        }
        let mut doc = toml_edit::DocumentMut::new();
        for (id, categories) in &self.entries {
            let mut array = toml_edit::Array::new();
            for category in categories {
                array.push(category.as_str());
            }
            doc[id.as_str()] = toml_edit::value(array);
        }
        std::fs::write(&path, doc.to_string())
            .map_err(|source| LocalrefError::io(&path, source))
    }

    /// Record that a junction for `(item_id, category)` has been projected.
    pub fn insert(&mut self, item_id: &str, category: &str) {
        let _ = self
            .entries
            .entry(item_id.to_string())
            .or_default()
            .insert(category.to_string());
    }

    /// Remove one projected `(item_id, category)` record.
    ///
    /// Empties are pruned so a manifest with no live memberships stays clean.
    pub fn remove(&mut self, item_id: &str, category: &str) {
        if let Some(categories) = self.entries.get_mut(item_id) {
            let _ = categories.remove(category);
            if categories.is_empty() {
                let _ = self.entries.remove(item_id);
            }
        }
    }

    /// Drop every record for one item (e.g. when the item is deleted).
    pub fn remove_item(&mut self, item_id: &str) {
        let _ = self.entries.remove(item_id);
    }

    /// Whether a junction for `(item_id, category)` is recorded as projected.
    #[must_use]
    pub fn contains(&self, item_id: &str, category: &str) -> bool {
        self.entries
            .get(item_id)
            .is_some_and(|categories| categories.contains(category))
    }

    /// Whether the manifest records no projected junctions at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::CatManifest;

    #[test]
    fn load_missing_file_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            CatManifest::load(temp.path()).unwrap(),
            CatManifest::default()
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let mut manifest = CatManifest::default();
        manifest.insert("lr:test:1", "Wireless/RIS");
        manifest.insert("lr:test:1", "Tagged");
        manifest.insert("lr:test:2", "Inbox");
        manifest.save(temp.path()).unwrap();

        let loaded = CatManifest::load(temp.path()).unwrap();
        assert_eq!(loaded, manifest);
        assert!(loaded.contains("lr:test:1", "Wireless/RIS"));
        assert!(loaded.contains("lr:test:2", "Inbox"));
        assert!(!loaded.contains("lr:test:2", "Wireless/RIS"));
    }

    #[test]
    fn remove_prunes_empty_items() {
        let mut manifest = CatManifest::default();
        manifest.insert("lr:test:1", "Inbox");
        manifest.remove("lr:test:1", "Inbox");
        assert!(!manifest.contains("lr:test:1", "Inbox"));
        // A fully-emptied item leaves the manifest equal to default.
        assert_eq!(manifest, CatManifest::default());
    }
}
