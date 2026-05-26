//! Query database for Localref libraries.
//!
//! `storage` owns the rebuildable query cache. The filesystem remains the
//! source of truth: `All/<item>/metadata.toml` is scanned into redb records, and
//! API queries read from that cache. If the database is deleted, a rescan can
//! rebuild it from `All/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{LocalrefError, Result};
use crate::model::Metadata;
pub use crate::model::{ItemDocument, SearchHit};
use crate::scan::{CatEntryKind, scan_cat};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

const ITEMS_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("items");

/// Rebuildable query database for one Localref library.
#[derive(Clone)]
pub struct StorageDb {
    library_root: PathBuf,
    database: Arc<Database>,
}

impl StorageDb {
    /// Open or create the query database rooted at `library/.localref/db`.
    pub fn open(library_root: impl Into<PathBuf>) -> Result<Self> {
        let library_root = library_root.into();
        let db_dir = library_root.join(".localref").join("db");
        fs::create_dir_all(&db_dir)
            .map_err(|source| LocalrefError::io(&db_dir, source))?;
        let db_path = db_dir.join("query.redb");
        let database = Database::create(&db_path)
            .or_else(|_| Database::open(&db_path))
            .map_err(|error| LocalrefError::Storage(error.to_string()))?;

        Ok(Self { library_root, database: Arc::new(database) })
    }

    /// Return the library root this database indexes.
    pub fn library_root(&self) -> &Path {
        &self.library_root
    }

    /// Rebuild item records from `All/*/metadata.toml`.
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
        write
            .commit()
            .map_err(|error| LocalrefError::Storage(error.to_string()))?;
        Ok(documents.len())
    }

    /// Return all indexed item documents.
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
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }

        let hits = self
            .list_items()?
            .into_iter()
            .filter(|item| item_matches(item, &needle))
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
#[derive(
    Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize,
)]
pub struct CategorySummary {
    /// Category path relative to `Cat/`.
    pub path: String,
    /// Item ids currently linked under this category.
    pub item_ids: Vec<String>,
}

fn scan_all_documents(library_root: &Path) -> Result<Vec<ItemDocument>> {
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
        let metadata = Metadata::from_toml_str(&metadata_text)?;
        documents.push(document_from_metadata(
            library_root,
            &item_dir,
            metadata,
            metadata_revision,
        ));
    }
    attach_categories(library_root, &mut documents)?;
    Ok(documents)
}

fn document_from_metadata(
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
        categories: Vec::new(),
    }
}

fn attach_categories(
    library_root: &Path,
    documents: &mut [ItemDocument],
) -> Result<()> {
    let cat_entries = scan_cat(library_root)?;
    let item_paths: Vec<_> = documents
        .iter()
        .map(|document| {
            (
                document.object_path.clone(),
                library_root.join(&document.object_path).canonicalize().ok(),
            )
        })
        .collect();

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
        let Some((document_index, _)) =
            item_paths.iter().enumerate().find(|(_, (_, item_path))| {
                item_path.is_some() && item_path == &target
            })
        else {
            continue;
        };
        let category = category.as_str().to_string();
        if !documents[document_index].categories.contains(&category) {
            documents[document_index].categories.push(category);
        }
    }

    for document in documents {
        document.categories.sort();
    }
    Ok(())
}

fn scan_category_directories(library_root: &Path) -> Result<Vec<String>> {
    let mut categories = scan_cat(library_root)?
        .into_iter()
        .filter(|entry| entry.kind == CatEntryKind::CategoryDirectory)
        .filter(|entry| {
            fs::read_dir(library_root.join(&entry.path))
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false)
        })
        .filter_map(|entry| {
            entry.path.strip_prefix("Cat/").map(str::to_string)
        })
        .collect::<Vec<_>>();
    categories.sort();
    categories.dedup();
    Ok(categories)
}

fn path_from_scan_target(library_root: &Path, target_path: &str) -> PathBuf {
    let target = PathBuf::from(target_path);
    if target.is_absolute() { target } else { library_root.join(target) }
}

fn item_matches(item: &ItemDocument, needle: &str) -> bool {
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
    fn rebuild_derives_categories_from_cat_links() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Paper One");
        fs::create_dir_all(&item_dir).unwrap();
        fs::write(
            item_dir.join("metadata.toml"),
            r#"
id = "lr:test:cat"
type = "journalArticle"
title = "Categorized Paper"
"#,
        )
        .unwrap();
        crate::platformfs::LibraryFs::new(temp.path())
            .create_category_link(
                &crate::types::CategoryPath::new("Wireless/RIS").unwrap(),
                &item_dir,
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
}
