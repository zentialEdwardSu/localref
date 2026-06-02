//! Serializable data model shared by Localref SSR and hydration.

use serde::{Deserialize, Serialize};

/// Complete Localref UI state for one URL route.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UiState {
    /// Repository display name rendered in the page chrome.
    pub repo_name: String,
    /// Search query in the current route.
    pub search: Option<String>,
    /// Category filter in the current route.
    pub category: Option<String>,
    /// Items visible after current browsing filters.
    pub items: Vec<ItemSummary>,
    /// Categories available in the library.
    pub categories: Vec<CategorySummary>,
    /// Recent daemon events.
    pub events: Vec<EventSummary>,
    /// Number of pending imports.
    pub pending_count: usize,
    /// Checkbox-selected item ids.
    pub selected_ids: Vec<String>,
    /// Item ids targeted by category controls.
    pub category_target_ids: Vec<String>,
    /// Active detail item id.
    pub active_id: Option<String>,
    /// Active metadata fields for the detail pane.
    pub active_detail: Option<ActiveDetail>,
    /// Active right-pane tab.
    pub tab: String,
    /// Return path preserving current server route state.
    pub return_to: String,
    /// Compact daemon status label.
    pub status_label: String,
    /// Whether watcher processing is paused.
    pub watcher_paused: bool,
    /// Files for the active item.
    pub files: Vec<FileEntry>,
    /// Current automatic-classification rules text.
    pub rules_text: String,
    /// Optional rules save result.
    pub rules_notice: Option<RulesNotice>,
}

/// Metadata fields for the active detail pane.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActiveDetail {
    /// Metadata revision used for optimistic save checks.
    pub metadata_revision: String,
    /// User-visible title.
    pub title: String,
    /// Semicolon-separated author summary.
    pub authors: String,
    /// Item type label.
    pub item_type: String,
    /// Publication year, when known.
    pub year: Option<i32>,
    /// DOI, when known.
    pub doi: Option<String>,
    /// Venue, when known.
    pub venue: Option<String>,
    /// Language, when known.
    pub language: Option<String>,
    /// URI, when known.
    pub uri: Option<String>,
    /// Abstract text, when known.
    pub abstract_note: Option<String>,
}

/// Item summary rendered in the library list.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ItemSummary {
    /// Stable Localref item id.
    pub id: String,
    /// User-visible title.
    pub title: String,
    /// User-visible author names.
    pub authors: Vec<String>,
    /// Item type label.
    pub item_type: String,
    /// Category paths assigned to the item.
    pub categories: Vec<String>,
    /// Main file path, when present.
    pub main_file: Option<String>,
    /// Files recorded in metadata for this item.
    #[serde(default)]
    pub files: Vec<String>,
}

/// Category summary rendered in filters and transfer controls.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CategorySummary {
    /// Category path.
    pub path: String,
    /// Number of items in the category.
    pub item_count: usize,
}

/// Recent daemon event rendered in the events panel.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EventSummary {
    /// Monotonic event id.
    pub id: u64,
    /// Event kind label.
    pub kind: String,
    /// User-visible message.
    pub message: String,
    /// Related item id, when present.
    pub item_id: Option<String>,
    /// Related library path, when present.
    pub path: Option<String>,
}

/// File entry rendered on the files tab.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FileEntry {
    /// Path relative to the item directory.
    pub path: String,
    /// Entry kind label.
    pub kind: String,
    /// File size in bytes, when present.
    pub bytes: Option<u64>,
    /// Whether this file is the metadata main file.
    #[serde(default)]
    pub is_main: bool,
}

/// Rules save notice rendered as a floating dialog.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RulesNotice {
    /// Rules parsed successfully.
    Saved {
        /// Parsed rules.
        rules: Vec<RuleSummary>,
    },
    /// Rules failed to parse or validate.
    Error {
        /// User-visible error message.
        message: String,
    },
}

/// One automatic-classification rule summary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuleSummary {
    /// Rule name.
    pub name: String,
    /// Target category path.
    pub target: String,
    /// Query expression.
    pub query: String,
}
