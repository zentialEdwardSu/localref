//! Query database for Localref libraries.
//!
//! `storage` owns the rebuildable query cache. The filesystem remains the
//! source of truth: `All/<item>/metadata.toml` is scanned into redb records, and
//! API queries read from that cache. If the database is deleted, a rescan can
//! rebuild it from `All/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{LocalrefError, Result};
use crate::model::Metadata;
pub use crate::model::{ItemDocument, SearchHit};
use crate::scan::{CatEntryKind, scan_cat};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

/// Internal helper for items table.
const ITEMS_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("items");

/// Secondary index over declared-indexed plugin `extra` fields.
///
/// Keyed by `"namespace.key"`; value is a JSON map of `field value` → sorted
/// unique item ids that carry it. Rebuilt from metadata alongside the items
/// table, and populated only for fields a plugin declared as indexed.
const EXTRA_INDEX_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("extra_index");

/// Rebuildable query database for one Localref library.
#[derive(Clone)]
pub struct StorageDb {
    /// Stored library root.
    library_root: PathBuf,
    /// Stored database.
    database: Arc<Database>,
    /// Plugin-declared indexed `extra` fields as `namespace.key`. Values of
    /// these fields participate in search and the secondary index. Shared across
    /// clones so a rescan updating the set is seen by every daemon clone (the
    /// servers, workers, and FFI search all hold clones).
    indexed_fields: Arc<RwLock<BTreeSet<String>>>,
}

impl StorageDb {
    /// Open or create the query database rooted at `library/.localref/db`.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn open(library_root: impl Into<PathBuf>) -> Result<Self> {
        let library_root = library_root.into();
        let db_dir = library_root.join(".localref").join("db");
        fs::create_dir_all(&db_dir)
            .map_err(|source| LocalrefError::io(&db_dir, source))?;
        let db_path = db_dir.join("query.redb");
        let database = Database::create(&db_path)
            .or_else(|_| Database::open(&db_path))
            .map_err(|error| LocalrefError::Storage(error.to_string()))?;

        Ok(Self {
            library_root,
            database: Arc::new(database),
            indexed_fields: Arc::new(RwLock::new(BTreeSet::new())),
        })
    }

    /// Return the library root this database indexes.
    #[must_use]
    pub fn library_root(&self) -> &Path {
        &self.library_root
    }

    /// Replace the set of plugin-declared indexed `extra` fields.
    ///
    /// Each entry is a `"namespace.key"` string. Callers pass the union of all
    /// discovered plugins' declared-indexed fields; a following
    /// [`Self::rebuild_from_all`] repopulates the secondary index accordingly.
    ///
    /// Takes `&self`: the set lives behind a shared lock so an update from a
    /// plugin rescan is observed by every [`StorageDb`] clone.
    pub fn set_indexed_fields(&self, fields: BTreeSet<String>) {
        *self
            .indexed_fields
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = fields;
    }

    /// The plugin-declared indexed `extra` fields (`namespace.key`).
    #[must_use]
    pub fn indexed_fields(&self) -> BTreeSet<String> {
        self.indexed_fields
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Rebuild item records from `All/*/metadata.toml`.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn rebuild_from_all(&self) -> Result<usize> {
        let documents = scan_all_documents(&self.library_root)?;
        let write = self
            .database
            .begin_write()
            .map_err(|error| LocalrefError::Storage(error.to_string()))?;
        let _ = write.delete_table(ITEMS_TABLE);
        {
            let mut table = write
                .open_table(ITEMS_TABLE)
                .map_err(|error| LocalrefError::Storage(error.to_string()))?;
            for document in &documents {
                let json = serde_json::to_vec(document)?;
                table.insert(document.id.as_str(), json.as_slice()).map_err(
                    |error| LocalrefError::Storage(error.to_string()),
                )?;
            }
        }
        // Rebuild the secondary extra index for declared-indexed fields.
        let _ = write.delete_table(EXTRA_INDEX_TABLE);
        {
            let index = build_extra_index(&documents, &self.indexed_fields());
            let mut table = write
                .open_table(EXTRA_INDEX_TABLE)
                .map_err(|error| LocalrefError::Storage(error.to_string()))?;
            for (field, values) in &index {
                let json = serde_json::to_vec(values)?;
                table.insert(field.as_str(), json.as_slice()).map_err(
                    |error| LocalrefError::Storage(error.to_string()),
                )?;
            }
        }
        write
            .commit()
            .map_err(|error| LocalrefError::Storage(error.to_string()))?;
        Ok(documents.len())
    }

    /// Return all indexed item documents.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn list_items(&self) -> Result<Vec<ItemDocument>> {
        let read = self
            .database
            .begin_read()
            .map_err(|error| LocalrefError::Storage(error.to_string()))?;
        let table = match read.open_table(ITEMS_TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => {
                return Ok(Vec::new());
            }
            Err(error) => {
                return Err(LocalrefError::Storage(error.to_string()));
            }
        };

        let mut items = Vec::new();
        for entry in table
            .iter()
            .map_err(|error| LocalrefError::Storage(error.to_string()))?
        {
            let (_key, value) = entry
                .map_err(|error| LocalrefError::Storage(error.to_string()))?;
            items.push(serde_json::from_slice(value.value())?);
        }
        items.sort_by(|left: &ItemDocument, right| {
            left.title.cmp(&right.title)
        });
        Ok(items)
    }

    /// Return one indexed item document by id.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn get_item(&self, id: &str) -> Result<Option<ItemDocument>> {
        let read = self
            .database
            .begin_read()
            .map_err(|error| LocalrefError::Storage(error.to_string()))?;
        let table = match read.open_table(ITEMS_TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => {
                return Err(LocalrefError::Storage(error.to_string()));
            }
        };
        let Some(value) = table
            .get(id)
            .map_err(|error| LocalrefError::Storage(error.to_string()))?
        else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(value.value())?))
    }

    /// Search indexed metadata with a simple case-insensitive substring query.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let indexed = self.indexed_fields();

        let hits = self
            .list_items()?
            .into_iter()
            .filter(|item| {
                item_matches(item, &needle)
                    || indexed_extra_matches(
                        item,
                        &needle,
                        &indexed,
                    )
            })
            .map(|item| SearchHit {
                id: item.id,
                title: item.title,
                authors: item.authors,
                object_path: item.object_path,
                abstract_note: item.abstract_note,
                doi: item.doi,
            })
            .collect();
        Ok(hits)
    }

    /// Return category paths derived from indexed item documents.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn list_categories(&self) -> Result<Vec<CategorySummary>> {
        let mut categories = Vec::<CategorySummary>::new();
        for category in scan_category_directories(&self.library_root)? {
            categories.push(CategorySummary {
                path: category,
                item_ids: Vec::new(),
            });
        }
        for item in self.list_items()? {
            for category in item.categories {
                match categories
                    .iter_mut()
                    .find(|entry| entry.path == category)
                {
                    Some(entry) => entry.item_ids.push(item.id.clone()),
                    None => categories.push(CategorySummary {
                        path: category,
                        item_ids: vec![item.id.clone()],
                    }),
                }
            }
        }
        categories.sort_by(|left, right| left.path.cmp(&right.path));
        for category in &mut categories {
            category.item_ids.sort();
            category.item_ids.dedup();
        }
        Ok(categories)
    }
}

/// Category summary derived from `Cat/` links.
pub use crate::model::CategorySummary;

/// Scan all metadata documents stored under `All/`.
///
/// # Errors
///
/// Returns an error when directories or metadata files cannot be read.
pub fn scan_all_documents(library_root: &Path) -> Result<Vec<ItemDocument>> {
    let all_dir = library_root.join("All");
    if !all_dir.exists() {
        return Ok(Vec::new());
    }

    let mut documents = Vec::new();
    for entry in fs::read_dir(&all_dir)
        .map_err(|source| LocalrefError::io(&all_dir, source))?
    {
        let entry =
            entry.map_err(|source| LocalrefError::io(&all_dir, source))?;
        let item_dir = entry.path();
        if !item_dir.is_dir() {
            continue;
        }
        let metadata_path = item_dir.join("metadata.toml");
        if !metadata_path.exists() {
            continue;
        }
        let metadata_text = fs::read_to_string(&metadata_path)
            .map_err(|source| LocalrefError::io(&metadata_path, source))?;
        let metadata_revision = Metadata::revision_for_text(&metadata_text);
        // A single malformed or invalid metadata.toml must not abort the whole
        // rebuild: skip the offending item (logging it) so the rest of the
        // library stays queryable and mutable.
        let metadata = match Metadata::from_toml_str(&metadata_text) {
            Ok(metadata) => metadata,
            Err(error) => {
                tracing::warn!(
                    target: "localref::storage",
                    path = %metadata_path.display(),
                    %error,
                    "skipping item with unreadable metadata.toml during rebuild",
                );
                continue;
            }
        };
        documents.push(document_from_metadata(
            library_root,
            &item_dir,
            metadata,
            metadata_revision,
        ));
    }
    Ok(documents)
}

/// Convert metadata and its item directory into an indexed document.
#[must_use]
pub fn document_from_metadata(
    library_root: &Path,
    item_dir: &Path,
    metadata: Metadata,
    metadata_revision: String,
) -> ItemDocument {
    let object_path = item_dir
        .strip_prefix(library_root)
        .unwrap_or(item_dir)
        .to_string_lossy()
        .replace('\\', "/");
    let authors = metadata.author_names();
    let files = metadata.files;
    let extra = metadata.extra;
    let mut categories = metadata.categories;
    categories.sort();
    categories.dedup();

    ItemDocument {
        id: metadata.id,
        object_path,
        metadata_revision,
        title: metadata.title,
        authors,
        abstract_note: metadata.abstract_note,
        item_type: metadata.item_type,
        doi: metadata.doi,
        uri: metadata.uri,
        main_file: files.main.clone(),
        extra_files: files.extra.into_iter().map(|file| file.path).collect(),
        tags: metadata.tags.items,
        venue: metadata.venue,
        year: metadata.year,
        categories,
        extra,
    }
}

/// Attach categories discovered from `Cat/` links to indexed documents.
///
/// # Errors
///
/// Returns an error when category links cannot be scanned.
/// Observe category membership as projected by live `Cat/` junctions.
///
/// Returns one `(item_id, category)` pair per `Cat/<category>/<name>` junction
/// that resolves into an indexed item's `All/` directory. Membership is matched
/// by canonicalized path, mirroring how the pre-metadata design derived
/// categories. Category membership is no longer *stored* here — it lives in
/// `metadata.toml` — but scan reconciliation needs to know which junctions are
/// currently present on disk to diff them against metadata and the manifest.
///
/// `items` supplies each candidate item's id and library-relative `object_path`.
///
/// # Errors
///
/// Returns an error when the category tree cannot be scanned.
pub fn scan_cat_memberships(
    library_root: &Path,
    items: &[(String, String)],
) -> Result<Vec<(String, String)>> {
    let cat_entries = scan_cat(library_root)?;
    let item_paths: Vec<_> = items
        .iter()
        .map(|(id, object_path)| {
            (
                id.clone(),
                library_root.join(object_path).canonicalize().ok(),
            )
        })
        .collect();

    let mut memberships = Vec::new();
    for entry in cat_entries
        .into_iter()
        .filter(|entry| entry.kind == CatEntryKind::ItemLink)
    {
        let Some(category) = entry.category else {
            continue;
        };
        let Some(target_path) = entry.target_path else {
            continue;
        };
        let target = path_from_scan_target(library_root, &target_path)
            .canonicalize()
            .ok();
        let Some((id, _)) = item_paths.iter().find(|(_, item_path)| {
            item_path.is_some() && item_path == &target
        }) else {
            continue;
        };
        memberships.push((id.clone(), category.as_str().to_string()));
    }
    memberships.sort();
    memberships.dedup();
    Ok(memberships)
}

/// Scan empty category directories that are not represented by item links.
///
/// # Errors
///
/// Returns an error when the category tree cannot be scanned.
pub fn scan_category_directories(library_root: &Path) -> Result<Vec<String>> {
    let mut categories = scan_cat(library_root)?
        .into_iter()
        .filter(|entry| entry.kind == CatEntryKind::CategoryDirectory)
        .filter(|entry| {
            fs::read_dir(library_root.join(&entry.path))
                .is_ok_and(|mut entries| entries.next().is_none())
        })
        .filter_map(|entry| {
            entry.path.strip_prefix("Cat/").map(str::to_string)
        })
        .collect::<Vec<_>>();
    categories.sort();
    categories.dedup();
    Ok(categories)
}

/// Resolve a scan target against a library root when it is relative.
#[must_use]
pub fn path_from_scan_target(
    library_root: &Path,
    target_path: &str,
) -> PathBuf {
    let target = PathBuf::from(target_path);
    if target.is_absolute() { target } else { library_root.join(target) }
}

/// Return whether an indexed item contains a normalized search term.
#[must_use]
pub fn item_matches(item: &ItemDocument, needle: &str) -> bool {
    item.id.to_lowercase().contains(needle)
        || item.title.to_lowercase().contains(needle)
        || item
            .authors
            .iter()
            .any(|author| author.to_lowercase().contains(needle))
        || item.abstract_note.as_deref().is_some_and(|abstract_note| {
            abstract_note.to_lowercase().contains(needle)
        })
        || item.item_type.to_lowercase().contains(needle)
        || item
            .doi
            .as_deref()
            .is_some_and(|doi| doi.to_lowercase().contains(needle))
        || item
            .uri
            .as_deref()
            .is_some_and(|uri| uri.to_lowercase().contains(needle))
        || item
            .venue
            .as_deref()
            .is_some_and(|venue| venue.to_lowercase().contains(needle))
        || item.year.is_some_and(|year| year.to_string().contains(needle))
        || item.tags.iter().any(|tag| tag.to_lowercase().contains(needle))
        || item
            .categories
            .iter()
            .any(|category| category.to_lowercase().contains(needle))
}

/// Return whether any declared-indexed `extra` field value matches the needle.
///
/// Only fields listed in `indexed_fields` (as `"namespace.key"`) are consulted,
/// so undeclared `extra` data stays out of search.
#[must_use]
pub fn indexed_extra_matches(
    item: &ItemDocument,
    needle: &str,
    indexed_fields: &BTreeSet<String>,
) -> bool {
    indexed_fields.iter().any(|field| {
        let Some((namespace, key)) = field.split_once('.') else {
            return false;
        };
        item.extra
            .get(namespace)
            .and_then(|ns| ns.get(key))
            .is_some_and(|value| value.to_lowercase().contains(needle))
    })
}

/// Build the secondary extra index: `"namespace.key"` → (value → item ids).
///
/// Only declared-indexed fields are included; values map to the sorted, unique
/// ids of items carrying them.
#[must_use]
pub fn build_extra_index(
    documents: &[ItemDocument],
    indexed_fields: &BTreeSet<String>,
) -> BTreeMap<String, BTreeMap<String, Vec<String>>> {
    let mut index: BTreeMap<String, BTreeMap<String, Vec<String>>> =
        BTreeMap::new();
    for field in indexed_fields {
        let Some((namespace, key)) = field.split_once('.') else {
            continue;
        };
        for document in documents {
            if let Some(value) =
                document.extra.get(namespace).and_then(|ns| ns.get(key))
            {
                index
                    .entry(field.clone())
                    .or_default()
                    .entry(value.clone())
                    .or_default()
                    .push(document.id.clone());
            }
        }
    }
    for values in index.values_mut() {
        for ids in values.values_mut() {
            ids.sort();
            ids.dedup();
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilds_and_searches_all_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Paper One");
        fs::create_dir_all(&item_dir).unwrap();
        fs::write(
            item_dir.join("metadata.toml"),
            r#"
id = "lr:test:1"
type = "journalArticle"
title = "Near Field RIS Paper"
abstract_note = "A paper about near field channel models."
doi = "10.1234/example"
uri = "https://example.test"

[[creators]]
role = "author"
given = "Ada"
family = "Lovelace"

[files]
main = "paper.pdf"

[[files.extra]]
path = "paper.pdf"
kind = "attachment"
"#,
        )
        .unwrap();

        let db = StorageDb::open(temp.path()).unwrap();
        assert_eq!(db.rebuild_from_all().unwrap(), 1);

        let items = db.list_items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].object_path, "All/Paper One");

        let hits = db.search("ris").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "lr:test:1");
        assert_eq!(hits[0].authors, vec!["Ada Lovelace".to_string()]);
        let abstract_hits = db.search("channel models").unwrap();
        assert_eq!(
            abstract_hits[0].abstract_note.as_deref(),
            Some("A paper about near field channel models.")
        );
        let author_hits = db.search("lovelace").unwrap();
        assert_eq!(author_hits[0].id, "lr:test:1");
    }

    #[test]
    fn rebuild_reads_categories_from_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Paper One");
        fs::create_dir_all(&item_dir).unwrap();
        // Categories now live in metadata.toml, not in Cat/ junctions. A rebuild
        // must reflect membership without any junction present.
        fs::write(
            item_dir.join("metadata.toml"),
            r#"
id = "lr:test:cat"
type = "journalArticle"
title = "Categorized Paper"
categories = ["Wireless/RIS"]
"#,
        )
        .unwrap();

        let db = StorageDb::open(temp.path()).unwrap();
        assert_eq!(db.rebuild_from_all().unwrap(), 1);

        let item = db.get_item("lr:test:cat").unwrap().unwrap();
        assert_eq!(item.categories, vec!["Wireless/RIS"]);
        assert_eq!(db.search("wireless").unwrap()[0].id, "lr:test:cat");
        let categories = db.list_categories().unwrap();
        assert_eq!(categories[0].path, "Wireless/RIS");
        assert_eq!(categories[0].item_ids, vec!["lr:test:cat"]);
    }

    #[test]
    fn list_categories_includes_empty_cat_directories() {
        let temp = tempfile::tempdir().unwrap();
        crate::platformfs::LibraryFs::new(temp.path())
            .ensure_layout()
            .unwrap();
        crate::platformfs::LibraryFs::new(temp.path())
            .create_category_dir(
                &crate::types::CategoryPath::new("Inbox/New").unwrap(),
            )
            .unwrap();
        let db = StorageDb::open(temp.path()).unwrap();

        let categories = db.list_categories().unwrap();

        assert_eq!(categories.len(), 1);
        assert_eq!(categories[0].path, "Inbox/New");
        assert!(categories[0].item_ids.is_empty());
    }

    #[test]
    fn list_categories_ignores_stale_item_dirs_but_keeps_real_subclasses() {
        let temp = tempfile::tempdir().unwrap();
        let fs = crate::platformfs::LibraryFs::new(temp.path());
        fs.ensure_layout().unwrap();
        let item_dir = temp.path().join("All").join("Paper");
        std::fs::create_dir_all(&item_dir).unwrap();
        std::fs::write(
            item_dir.join("metadata.toml"),
            r#"
id = "lr:test:paper"
type = "document"
title = "Paper"
"#,
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("Cat/A/Paper")).unwrap();
        std::fs::create_dir_all(temp.path().join("Cat/A/Sub")).unwrap();
        let db = StorageDb::open(temp.path()).unwrap();

        let paths = db
            .list_categories()
            .unwrap()
            .into_iter()
            .map(|category| category.path)
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec!["A/Sub"],
            "copied item-link artifacts must not become subclasses, while real subclasses remain visible",
        );
    }
}
