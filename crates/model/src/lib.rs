//! Persistent Localref domain models.
//!
//! This crate owns structures that are written to disk or returned by user
//! APIs. Filesystem orchestration stays in `core`, while this crate defines the
//! metadata, query document, search result, and event shapes shared by those
//! layers.

use std::collections::BTreeMap;

use error::{LocalrefError, Result};
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
        if let Some(name) = optional_trimmed(&self.name) {
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

/// Durable daemon event written to `.localref/logs/events.jsonl`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Event {
    /// Monotonic event id inside the event log file.
    pub id: u64,
    /// Event kind.
    pub kind: EventKind,
    /// Human-readable event message.
    pub message: String,
    /// Related item id, when known.
    pub item_id: Option<String>,
    /// Related library-relative path, when known.
    pub path: Option<String>,
}

/// Event kind vocabulary for phase-one daemon behavior.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
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
            .filter(|creator| is_author_role(&creator.role))
            .filter_map(Creator::display_name)
            .collect()
    }

    /// Parse and validate metadata TOML text.
    pub fn from_toml_str(text: &str) -> Result<Self> {
        reject_categories_field(text)?;
        let metadata: Self = toml::from_str(text)?;
        metadata.validate()?;
        Ok(metadata)
    }

    /// Serialize metadata to pretty TOML after validation.
    pub fn to_toml_string(&self) -> Result<String> {
        self.validate()?;
        Ok(toml::to_string_pretty(self)?)
    }

    /// Validate required metadata invariants.
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
    pub fn revision_for_text(text: &str) -> String {
        stable_hash_hex(text.as_bytes())
    }
}

impl Event {
    /// Construct one event log record.
    pub fn new(
        id: u64,
        kind: EventKind,
        message: impl Into<String>,
        item_id: Option<String>,
        path: Option<String>,
    ) -> Self {
        Self { id, kind, message: message.into(), item_id, path }
    }
}

fn reject_categories_field(text: &str) -> Result<()> {
    let value: toml::Value = toml::from_str(text)?;
    if value.get("categories").is_some() {
        return Err(LocalrefError::Unsupported(
            "metadata.toml must not contain categories",
        ));
    }
    Ok(())
}

fn optional_trimmed(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn is_author_role(role: &str) -> bool {
    role.to_ascii_lowercase().contains("author")
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
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
