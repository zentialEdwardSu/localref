//! Shared Localref value types used across crate boundaries.
//!
//! This crate intentionally contains small data structures without filesystem
//! side effects. Pipeline orchestration belongs to `core`, while path creation
//! and NTFS-aware writes belong to `platformfs`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable identifier for a Localref literature item.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct ItemId(String);

impl ItemId {
    /// Create an item id from a non-empty string.
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        if value.trim().is_empty() { None } else { Some(Self(value)) }
    }

    /// Return the item id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Root path of one Localref library.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryRoot(PathBuf);

impl LibraryRoot {
    /// Create a library root from a path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// Return the root path.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Return the `All/` directory path.
    pub fn all_dir(&self) -> PathBuf {
        self.0.join("All")
    }

    /// Return the `Cat/` directory path.
    pub fn cat_dir(&self) -> PathBuf {
        self.0.join("Cat")
    }

    /// Return the `.localref/` internal state directory path.
    pub fn state_dir(&self) -> PathBuf {
        self.0.join(".localref")
    }
}

/// Category path relative to the `Cat/` root.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct CategoryPath(String);

impl CategoryPath {
    /// Create a category path from a non-empty slash-separated string.
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let trimmed = value.trim().trim_matches('/').replace('\\', "/");
        if trimmed.is_empty()
            || trimmed.split('/').any(|part| part.trim().is_empty())
        {
            None
        } else {
            Some(Self(trimmed))
        }
    }

    /// Return the category as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return path components separated by `/`.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

/// Bibliographic item payload received from Zotero Connector.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ConnectorItem {
    /// Zotero save session identifier, when supplied.
    pub session_id: Option<String>,
    /// Source page URI, when supplied.
    pub uri: Option<String>,
    /// Zotero item id/key from the translated item payload, when supplied.
    pub connector_item_id: Option<String>,
    /// Zotero item type, such as `journalArticle`.
    pub item_type: Option<String>,
    /// Literature title. This is required by the first filesystem import slice.
    pub title: String,
    /// Abstract text from Zotero Connector, when supplied.
    pub abstract_note: Option<String>,
    /// DOI value, when supplied by the translator.
    pub doi: Option<String>,
    /// Raw translated Zotero JSON item.
    pub raw: Value,
}

/// Attachment bytes uploaded by Zotero Connector.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ConnectorAttachment {
    /// Zotero save session identifier, when supplied.
    pub session_id: Option<String>,
    /// Parent item id/key, when supplied by attachment metadata.
    pub parent_item_id: Option<String>,
    /// User-visible attachment title.
    pub title: Option<String>,
    /// Filename Localref should use inside `All/<item>/`.
    pub filename: String,
    /// MIME type supplied by Zotero Connector.
    pub mime_type: Option<String>,
    /// Uploaded attachment bytes.
    pub bytes: Vec<u8>,
    /// Raw `X-Metadata` header decoded as JSON.
    pub raw_metadata: Option<Value>,
}

/// Connector import command accepted by `core::ImportPipeline`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ConnectorImport {
    /// Main translated item payload.
    pub item: ConnectorItem,
    /// Attachments already received for this item.
    pub attachments: Vec<ConnectorAttachment>,
}

/// Result of importing one connector item into `All/`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ImportOutcome {
    /// Generated Localref item id.
    pub item_id: ItemId,
    /// Directory created under `All/`.
    pub item_dir: PathBuf,
    /// Files written inside the item directory.
    pub written_files: Vec<PathBuf>,
    /// Categories linked for this imported item.
    pub categories: Vec<CategoryPath>,
}
