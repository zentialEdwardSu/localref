//! Persistent Localref domain models.
//!
//! This crate owns structures that are written to disk or returned by user
//! APIs. Filesystem orchestration stays in `core`, while this crate defines the
//! metadata, query document, search result, and event shapes shared by those
//! layers.

use std::collections::BTreeMap;

use crate::error::{LocalrefError, Result};
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
    /// Import is waiting for user category confirmation.
    ImportPendingUserConfirmation,
    /// A pending import was cancelled.
    ImportCancelled,
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
            LogKind::ImportPendingUserConfirmation => {
                "import_pending_user_confirmation"
            }
            LogKind::ImportCancelled => "import_cancelled",
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
        let value: toml::Value = toml::from_str(text)?;
        if value.get("categories").is_some() {
            return Err(LocalrefError::Unsupported(
                "metadata.toml must not contain categories",
            ));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_and_rejects_categories() {
        let metadata = Metadata::from_toml_str(
            r#"
id = "lr:test:1"
type = "journalArticle"
title = "A Paper"
abstract = "A short abstract"

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
        assert!(metadata.to_toml_string().unwrap().contains("journalArticle"));
        assert!(
            Metadata::from_toml_str(
                r#"
id = "lr:test:1"
type = "journalArticle"
title = "A Paper"
categories = ["Bad"]

[files]
main = "paper.pdf"

[import]
source = "manual"
"#,
            )
            .is_err()
        );
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
