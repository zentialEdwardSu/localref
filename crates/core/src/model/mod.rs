//! Persistent Localref domain models.
//!
//! This crate owns structures that are written to disk or returned by user
//! APIs. Filesystem orchestration stays in `core`, while this crate defines the
//! metadata, query document, search result, and event shapes shared by those
//! layers.

use std::collections::BTreeMap;

use crate::error::{LocalrefError, Result};
use crate::types::CategoryPath;
use serde::{Deserialize, Serialize};

/// Metadata stored in `All/<item>/metadata.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Metadata {
    /// Stable Localref item id.
    pub id: String,
    /// Zotero-compatible or Localref item type.
    #[serde(rename = "type")]
    pub item_type: String,
    /// User-visible literature title.
    pub title: String,
    /// Article or item abstract.
    #[serde(rename = "abstract", alias = "abstract_note")]
    pub abstract_note: Option<String>,
    /// DOI, when known.
    pub doi: Option<String>,
    /// Source URI, when known.
    pub uri: Option<String>,
    /// Publication year, when known.
    pub year: Option<i32>,
    /// Venue or container title, when known.
    pub venue: Option<String>,
    /// Language code or label, when known.
    pub language: Option<String>,
    /// Creators such as authors or editors.
    #[serde(default)]
    pub creators: Vec<Creator>,
    /// Category paths this item belongs to. Source of truth for membership; the
    /// `Cat/` junctions are a projection of this list reconciled during scan.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Files stored inside the item directory.
    #[serde(default)]
    pub files: MetadataFiles,
    /// Tag metadata stored on the item.
    #[serde(default)]
    pub tags: MetadataTags,
    /// Import provenance.
    #[serde(default)]
    pub import: MetadataImport,
    /// Item state flags.
    #[serde(default)]
    pub state: MetadataState,
    /// Connector-specific raw data preserved for future richer mappings.
    #[serde(default)]
    pub raw_connector: BTreeMap<String, String>,
    /// Plugin-owned per-item data, keyed by plugin namespace then field key.
    ///
    /// Each plugin writes under its own `[extra.<namespace>]` table. Values are
    /// strings; structured data is JSON-encoded by the plugin (as
    /// `raw_connector` does with `raw_json`). Plugins may declare specific
    /// `namespace.key` pairs as indexed in their manifest, which makes those
    /// values participate in search; undeclared entries are preserved but not
    /// searched.
    #[serde(default)]
    pub extra: BTreeMap<String, BTreeMap<String, String>>,
}

/// Person or organization associated with a metadata record.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Creator {
    /// Creator role, such as `author`.
    pub role: String,
    /// Given name, when structured creator data is available.
    pub given: Option<String>,
    /// Family name, when structured creator data is available.
    pub family: Option<String>,
    /// Single-field creator name.
    pub name: Option<String>,
}

impl Creator {
    /// Return the best user-visible name for this creator.
    pub fn display_name(&self) -> Option<String> {
        if let Some(name) = self
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        {
            return Some(name);
        }
        let parts = [self.given.as_deref(), self.family.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.is_empty() { None } else { Some(parts.join(" ")) }
    }
}

/// Files associated with a metadata record.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct MetadataFiles {
    /// Main file path relative to `All/<item>/`.
    pub main: Option<String>,
    /// Extra file records relative to `All/<item>/`.
    #[serde(default)]
    pub extra: Vec<MetadataFile>,
}

/// One file inside an item directory.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct MetadataFile {
    /// Relative path inside `All/<item>/`.
    pub path: String,
    /// File role, such as `attachment` or `source_url`.
    pub kind: String,
    /// MIME type, when known.
    pub mime_type: Option<String>,
}

/// Tags stored on the item.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct MetadataTags {
    /// Tag names.
    #[serde(default)]
    pub items: Vec<String>,
}

/// Import provenance stored in metadata.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct MetadataImport {
    /// Import source, such as `zotero-connector`.
    pub source: String,
    /// Connector save session id, when the source supplies one.
    pub session_id: Option<String>,
    /// Optional import timestamp string.
    pub imported_at: Option<String>,
}

impl Default for MetadataImport {
    fn default() -> Self {
        Self {
            source: "unknown".to_string(),
            session_id: None,
            imported_at: None,
        }
    }
}

/// State flags stored in metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct MetadataState {
    /// Whether the main file is missing.
    #[serde(default)]
    pub missing_main_file: bool,
    /// Category paths the user explicitly removed. Tombstones suppress
    /// auto-classification (rules, re-import) from re-filing these categories;
    /// an explicit add-category clears the matching tombstone.
    #[serde(default)]
    pub removed_categories: Vec<String>,
}

/// Item document stored in the query database.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ItemDocument {
    /// Stable Localref item id.
    pub id: String,
    /// Relative object path under the library root, such as `All/Paper`.
    pub object_path: String,
    /// Revision hash of the source `metadata.toml` text.
    pub metadata_revision: String,
    /// Literature title.
    pub title: String,
    /// User-visible creator names.
    #[serde(default)]
    pub authors: Vec<String>,
    /// Abstract text, when present.
    pub abstract_note: Option<String>,
    /// Item type from `metadata.toml`.
    pub item_type: String,
    /// DOI, when present.
    pub doi: Option<String>,
    /// Source URI, when present.
    pub uri: Option<String>,
    /// Main file path relative to the item directory, when present.
    pub main_file: Option<String>,
    /// Extra files relative to the item directory.
    pub extra_files: Vec<String>,
    /// Tag names.
    pub tags: Vec<String>,
    /// Venue or container title, when present.
    pub venue: Option<String>,
    /// Publication year, when present.
    pub year: Option<i32>,
    /// Category paths derived from `Cat/`.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Plugin-owned per-item data carried through from `metadata.toml`.
    #[serde(default)]
    pub extra: BTreeMap<String, BTreeMap<String, String>>,
}

/// Files currently present under one item directory.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ItemFilesDocument {
    /// Stable Localref item id.
    pub item_id: String,
    /// Relative object path under the library root, such as `All/Paper`.
    pub object_path: String,
    /// Files and directories inside the item directory.
    #[serde(default)]
    pub files: Vec<ItemFileEntry>,
}

/// One filesystem entry inside an item directory.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ItemFileEntry {
    /// Path relative to the item directory.
    pub path: String,
    /// Entry kind: `file`, `directory`, or `other`.
    pub kind: String,
    /// File size in bytes when the entry is a regular file.
    pub bytes: Option<u64>,
}

/// Full metadata payload paired with its source revision.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct MetadataDocument {
    /// Stable Localref item id.
    pub item_id: String,
    /// Revision hash of the source `metadata.toml` text.
    pub metadata_revision: String,
    /// Parsed metadata document.
    pub metadata: Metadata,
}

/// Search result returned by query APIs.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct SearchHit {
    /// Matching item id.
    pub id: String,
    /// Matching item title.
    pub title: String,
    /// User-visible creator names that participate in search.
    #[serde(default)]
    pub authors: Vec<String>,
    /// Relative object path under the library root.
    pub object_path: String,
    /// Optional DOI.
    pub doi: Option<String>,
    /// Optional abstract snippet.
    pub abstract_note: Option<String>,
}

/// Category summary derived from `Cat/` links.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct CategorySummary {
    /// Category path relative to `Cat/`.
    pub path: String,
    /// Item ids currently linked under this category.
    pub item_ids: Vec<String>,
}

/// A runtime-registered scheduled plugin call.
///
/// Persisted to `<library>/.localref/schedules.toml` and fired by the daemon's
/// cron scheduler. The target `plugin` may be the registering plugin itself or
/// any other discovered plugin, invoked as `run <action>` with `params`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ScheduledCall {
    /// Unique schedule id; used as the delete key.
    pub id: String,
    /// Target plugin name (self or any other discovered plugin).
    pub plugin: String,
    /// Action id passed to the target plugin as `run <action>`.
    pub action: String,
    /// Parameters forwarded to the action as `--param key=value`.
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    /// Cron expression (6 fields: sec min hour day-of-month month day-of-week).
    pub schedule: String,
}

/// Well-known log event kind identifiers used as `event_kind` field values.
///
/// Each variant serializes to its `snake_case` string form, matching the values
/// previously emitted by the old `EventKind` enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogKind {
    /// An import task started.
    ImportStarted,
    /// An import task finished.
    ImportFinished,
    /// An import task failed.
    ImportFailed,
    /// An item was registered in `All/`.
    ItemRegistered,
    /// A metadata file was created or rewritten.
    MetadataWritten,
    /// A scan task started.
    ScanStarted,
    /// A scan task finished.
    ScanFinished,
    /// A pause mode changed.
    PauseChanged,
    /// A lock conflict prevented a write.
    WriteConflict,
    /// Import-time rules matched one or more categories.
    AutoClassifiedOnImport,
    /// A category link was created in `Cat/`.
    CatLinkCreated,
    /// A category directory was created in `Cat/`.
    CategoryCreated,
    /// Metadata was created for an existing `All/` directory.
    MetadataCreated,
    /// Multiple candidate main files need user selection.
    MultipleMainPdfCandidates,
    /// A real directory under `Cat/` was normalized.
    CatCopyReplacedByLink,
    /// A category link was deleted from `Cat/`.
    CatLinkDeleted,
    /// A category directory was renamed.
    CategoryRenamed,
    /// A category directory was merged into another category.
    CategoryMerged,
    /// An indexed item directory was deleted from `All/`.
    ItemDeleted,
}

impl LogKind {
    /// Return the canonical `snake_case` string for this variant.
    ///
    /// The returned string matches the value written into the JSONL
    /// `event_kind` field and the old `EventKind` serialized names.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            LogKind::ImportStarted => "import_started",
            LogKind::ImportFinished => "import_finished",
            LogKind::ImportFailed => "import_failed",
            LogKind::ItemRegistered => "item_registered",
            LogKind::MetadataWritten => "metadata_written",
            LogKind::ScanStarted => "scan_started",
            LogKind::ScanFinished => "scan_finished",
            LogKind::PauseChanged => "pause_changed",
            LogKind::WriteConflict => "write_conflict",
            LogKind::AutoClassifiedOnImport => "auto_classified_on_import",
            LogKind::CatLinkCreated => "cat_link_created",
            LogKind::CategoryCreated => "category_created",
            LogKind::MetadataCreated => "metadata_created",
            LogKind::MultipleMainPdfCandidates => {
                "multiple_main_pdf_candidates"
            }
            LogKind::CatCopyReplacedByLink => "cat_copy_replaced_by_link",
            LogKind::CatLinkDeleted => "cat_link_deleted",
            LogKind::CategoryRenamed => "category_renamed",
            LogKind::CategoryMerged => "category_merged",
            LogKind::ItemDeleted => "item_deleted",
        }
    }
}

impl Metadata {
    /// Return user-visible creator names in metadata order.
    pub fn creator_names(&self) -> Vec<String> {
        self.creators.iter().filter_map(Creator::display_name).collect()
    }

    /// Return user-visible author names in metadata order.
    pub fn author_names(&self) -> Vec<String> {
        self.creators
            .iter()
            .filter(|creator| {
                creator.role.to_ascii_lowercase().contains("author")
            })
            .filter_map(Creator::display_name)
            .collect()
    }

    /// Parse and validate metadata TOML text.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn from_toml_str(text: &str) -> Result<Self> {
        let metadata: Self = toml::from_str(text)?;
        metadata.validate()?;
        Ok(metadata)
    }

    /// Serialize metadata to pretty TOML after validation.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn to_toml_string(&self) -> Result<String> {
        self.validate()?;
        Ok(toml::to_string_pretty(self)?)
    }

    /// Apply category membership edits to existing `metadata.toml` text in place.
    ///
    /// - `add`: ensured present in the top-level `categories` array and cleared
    ///   from the `[state].removed_categories` tombstone list.
    /// - `remove`: dropped from `categories` and recorded as tombstones so
    ///   auto-classification will not re-file them (a deliberate user removal).
    /// - `drop`: dropped from `categories` **without** tombstoning — used for
    ///   internal remaps (rename/merge) where the old path is simply moving.
    ///
    /// Formatting and comments elsewhere in the document are preserved because
    /// the edit uses `toml_edit` rather than reserializing. The lists are applied
    /// in order (`add`, then `remove`, then `drop`); pass disjoint sets.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not valid TOML.
    ///
    /// # Panics
    ///
    /// Panics if the `toml_edit` table invariant established in this method is
    /// violated internally.
    pub fn apply_category_edits(
        text: &str,
        add: &[&str],
        remove: &[&str],
        drop: &[&str],
    ) -> Result<String> {
        let mut doc = text.parse::<toml_edit::DocumentMut>()?;

        ensure_string_array(doc.as_table_mut(), "categories");
        for category in add {
            array_insert(&mut doc["categories"], category);
        }
        for category in remove.iter().chain(drop) {
            array_remove(&mut doc["categories"], category);
        }

        let state_entry = doc.entry("state").or_insert_with(|| {
            toml_edit::Item::Table(toml_edit::Table::new())
        });
        // Tombstones live in `[state].removed_categories`. If `state` somehow is
        // not a table (hand-edited scalar, or a future schema), replace it with
        // a fresh table so a `remove` is always tombstoned — otherwise the
        // category would be dropped from `categories` but silently re-filed by
        // auto-classification on the next scan.
        if !state_entry.is_table() {
            *state_entry = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let state = state_entry
            .as_table_mut()
            .expect("state was just ensured to be a table");
        state.set_implicit(false);
        ensure_string_array(state, "removed_categories");
        for category in add.iter().chain(drop) {
            array_remove(&mut state["removed_categories"], category);
        }
        for category in remove {
            array_insert(&mut state["removed_categories"], category);
        }

        Ok(doc.to_string())
    }

    /// Set one plugin `extra` value in existing `metadata.toml` text in place.
    ///
    /// Writes `[extra.<namespace>] <key> = <value>`, creating the tables as
    /// needed and preserving unrelated formatting and comments (the edit uses
    /// `toml_edit`). Passing `None` for `value` removes the key, and prunes the
    /// namespace table when it becomes empty.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not valid TOML.
    pub fn apply_extra_edit(
        text: &str,
        namespace: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<String> {
        let mut doc = text.parse::<toml_edit::DocumentMut>()?;

        let extra = doc
            .entry("extra")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
        let Some(extra) = extra.as_table_mut() else {
            return Err(LocalrefError::InvalidPathComponent {
                component: "extra".to_string(),
                reason: "metadata.extra is not a table",
            });
        };
        extra.set_implicit(true);

        match value {
            Some(value) => {
                let ns = extra.entry(namespace).or_insert(
                    toml_edit::Item::Table(toml_edit::Table::new()),
                );
                if let Some(ns) = ns.as_table_mut() {
                    ns.insert(key, toml_edit::value(value));
                }
            }
            None => {
                if let Some(ns) = extra
                    .get_mut(namespace)
                    .and_then(toml_edit::Item::as_table_mut)
                {
                    let _ = ns.remove(key);
                    if ns.is_empty() {
                        let _ = extra.remove(namespace);
                    }
                }
            }
        }

        Ok(doc.to_string())
    }

    /// Validate required metadata invariants.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(LocalrefError::MissingField("metadata.id"));
        }
        if self.title.trim().is_empty() {
            return Err(LocalrefError::MissingField("metadata.title"));
        }
        for category in &self.categories {
            if CategoryPath::new(category.as_str()).is_none() {
                return Err(LocalrefError::InvalidPathComponent {
                    component: category.clone(),
                    reason: "metadata.categories entry is not a valid category",
                });
            }
        }
        Ok(())
    }

    /// Return a stable revision hash for TOML text.
    #[must_use]
    pub fn revision_for_text(text: &str) -> String {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
}

/// Ensure `table[key]` is an array, creating an empty one when absent or when
/// the existing value is a different type.
fn ensure_string_array(table: &mut toml_edit::Table, key: &str) {
    if !table.get(key).is_some_and(toml_edit::Item::is_array) {
        table.insert(key, toml_edit::value(toml_edit::Array::new()));
    }
}

/// Append `value` to a TOML array item if not already present.
fn array_insert(item: &mut toml_edit::Item, value: &str) {
    let Some(array) = item.as_array_mut() else {
        return;
    };
    if !array.iter().any(|entry| entry.as_str() == Some(value)) {
        array.push(value);
    }
}

/// Remove every occurrence of `value` from a TOML array item.
fn array_remove(item: &mut toml_edit::Item, value: &str) {
    if let Some(array) = item.as_array_mut() {
        array.retain(|entry| entry.as_str() != Some(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_and_reads_categories() {
        let metadata = Metadata::from_toml_str(
            r#"
id = "lr:test:1"
type = "journalArticle"
title = "A Paper"
abstract = "A short abstract"
categories = ["Wireless/RIS"]

[files]
main = "paper.pdf"

[tags]
items = ["RIS"]

[import]
source = "manual-all-directory"
"#,
        )
        .unwrap();

        assert_eq!(
            metadata.abstract_note.as_deref(),
            Some("A short abstract")
        );
        assert_eq!(metadata.categories, vec!["Wireless/RIS".to_string()]);
        assert!(metadata.to_toml_string().unwrap().contains("journalArticle"));

        // An invalid category string is rejected by validation.
        assert!(
            Metadata::from_toml_str(
                r#"
id = "lr:test:1"
type = "journalArticle"
title = "A Paper"
categories = ["/"]

[import]
source = "manual"
"#,
            )
            .is_err()
        );
    }

    #[test]
    fn apply_category_edits_preserves_comments_and_toggles_tombstones() {
        let original = r#"# item file
id = "lr:test:1"
type = "journalArticle"
title = "A Paper" # inline comment

[import]
source = "manual"
"#;

        // Add a category: appears in categories, no tombstone, comments kept.
        let added = Metadata::apply_category_edits(
            original,
            &["Wireless/RIS"],
            &[],
            &[],
        )
        .unwrap();
        assert!(added.contains("# item file"));
        assert!(added.contains("# inline comment"));
        let parsed = Metadata::from_toml_str(&added).unwrap();
        assert_eq!(parsed.categories, vec!["Wireless/RIS".to_string()]);
        assert!(parsed.state.removed_categories.is_empty());

        // Remove it: dropped from categories, recorded as a tombstone.
        let removed = Metadata::apply_category_edits(
            &added,
            &[],
            &["Wireless/RIS"],
            &[],
        )
        .unwrap();
        let parsed = Metadata::from_toml_str(&removed).unwrap();
        assert!(parsed.categories.is_empty());
        assert_eq!(
            parsed.state.removed_categories,
            vec!["Wireless/RIS".to_string()]
        );

        // Re-add: clears the tombstone.
        let readded = Metadata::apply_category_edits(
            &removed,
            &["Wireless/RIS"],
            &[],
            &[],
        )
        .unwrap();
        let parsed = Metadata::from_toml_str(&readded).unwrap();
        assert_eq!(parsed.categories, vec!["Wireless/RIS".to_string()]);
        assert!(parsed.state.removed_categories.is_empty());

        // Drop (rename/merge): removed from categories WITHOUT a tombstone.
        let dropped = Metadata::apply_category_edits(
            &readded,
            &[],
            &[],
            &["Wireless/RIS"],
        )
        .unwrap();
        let parsed = Metadata::from_toml_str(&dropped).unwrap();
        assert!(parsed.categories.is_empty());
        assert!(parsed.state.removed_categories.is_empty());
    }

    #[test]
    fn apply_category_edits_tombstones_even_when_state_is_not_a_table() {
        // A `state` that is not a table (hand-edited, or a stale/other schema)
        // must not cause a `remove` to skip its tombstone: otherwise the
        // category is dropped from `categories` but auto-classification silently
        // re-files it on the next scan.
        let original = r#"id = "lr:test:1"
type = "journalArticle"
title = "A Paper"
categories = ["Wireless/RIS"]
state = "unexpected-scalar"
"#;

        let removed = Metadata::apply_category_edits(
            original,
            &[],
            &["Wireless/RIS"],
            &[],
        )
        .unwrap();
        let parsed = Metadata::from_toml_str(&removed).unwrap();
        assert!(parsed.categories.is_empty());
        assert_eq!(
            parsed.state.removed_categories,
            vec!["Wireless/RIS".to_string()],
            "removal must be tombstoned even when [state] started as a scalar",
        );
    }

    #[test]
    fn extra_round_trips_through_metadata() {
        let metadata = Metadata::from_toml_str(
            r#"
id = "lr:test:1"
type = "document"
title = "A Paper"

[extra.bibtexer]
cite_key = "smith2020"

[extra.rating]
stars = "5"
"#,
        )
        .unwrap();
        assert_eq!(
            metadata.extra["bibtexer"]["cite_key"],
            "smith2020".to_string()
        );
        assert_eq!(metadata.extra["rating"]["stars"], "5".to_string());
        // Serializing and reparsing preserves the values.
        let text = metadata.to_toml_string().unwrap();
        let reparsed = Metadata::from_toml_str(&text).unwrap();
        assert_eq!(reparsed.extra, metadata.extra);
    }

    #[test]
    fn apply_extra_edit_preserves_comments_and_prunes_empty() {
        let original = r#"# item file
id = "lr:test:1"
type = "document"
title = "A Paper" # inline comment
"#;

        // Set a value: table is created, comments kept.
        let set = Metadata::apply_extra_edit(
            original,
            "bibtexer",
            "cite_key",
            Some("smith2020"),
        )
        .unwrap();
        assert!(set.contains("# item file"));
        assert!(set.contains("# inline comment"));
        let parsed = Metadata::from_toml_str(&set).unwrap();
        assert_eq!(parsed.extra["bibtexer"]["cite_key"], "smith2020");

        // Remove it: the now-empty namespace table is pruned.
        let cleared =
            Metadata::apply_extra_edit(&set, "bibtexer", "cite_key", None)
                .unwrap();
        let parsed = Metadata::from_toml_str(&cleared).unwrap();
        assert!(parsed.extra.get("bibtexer").is_none());
        assert!(cleared.contains("# inline comment"));
    }

    #[test]
    fn revision_is_stable_for_same_text() {
        assert_eq!(
            Metadata::revision_for_text("abc"),
            Metadata::revision_for_text("abc")
        );
        assert_ne!(
            Metadata::revision_for_text("abc"),
            Metadata::revision_for_text("abcd")
        );
    }

    #[test]
    fn creator_names_prefer_single_field_and_join_structured_names() {
        let metadata = Metadata::from_toml_str(
            r#"
id = "lr:test:1"
type = "journalArticle"
title = "A Paper"

[[creators]]
role = "editor"
name = "Near Field Group"

[[creators]]
role = "bookAuthor"
given = "Ada"
family = "Lovelace"
"#,
        )
        .unwrap();

        assert_eq!(
            metadata.creator_names(),
            vec!["Near Field Group".to_string(), "Ada Lovelace".to_string()]
        );
        assert_eq!(metadata.author_names(), vec!["Ada Lovelace".to_string()]);
    }
}
