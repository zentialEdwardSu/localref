//! Serializable state types shared between the host and plugins.

use serde::{Deserialize, Serialize};

/// Complete UI state passed to plugins during render and run invocations.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginUiState {
    /// Repository display name.
    pub repo_name: String,
    /// Current search query from the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    /// Current category filter from the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Items visible after applying browsing filters.
    pub items: Vec<PluginItemSummary>,
    /// All categories available in the library.
    pub categories: Vec<PluginCategorySummary>,
    /// Checkbox-selected item ids.
    #[serde(default)]
    pub selected_ids: Vec<String>,
    /// Active detail item id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_id: Option<String>,
    /// Active item metadata fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_detail: Option<PluginActiveDetail>,
    /// Active right-pane tab.
    pub tab: String,
    /// Compact daemon status label.
    pub status_label: String,
    /// Absolute path to the library root.
    pub library_root: String,
    /// REST API endpoint for callbacks from plugin JS.
    pub rest_endpoint: String,
}

/// Item summary visible to plugins.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginItemSummary {
    /// Stable Localref item id.
    pub id: String,
    /// User-visible title.
    pub title: String,
    /// Author names.
    #[serde(default)]
    pub authors: Vec<String>,
    /// Item type label.
    pub item_type: String,
    /// Category paths assigned to the item.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Main file path, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_file: Option<String>,
    /// All recorded file paths for this item.
    #[serde(default)]
    pub files: Vec<String>,
}

/// Category summary visible to plugins.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginCategorySummary {
    /// Category path relative to Cat/.
    pub path: String,
    /// Number of items in the category.
    pub item_count: usize,
}

/// Active detail metadata visible to plugins.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginActiveDetail {
    /// Metadata revision for optimistic save checks.
    pub metadata_revision: String,
    /// User-visible title.
    pub title: String,
    /// Semicolon-separated author summary.
    pub authors: String,
    /// Item type label.
    pub item_type: String,
    /// Publication year, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    /// DOI, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
    /// Venue, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub venue: Option<String>,
    /// Language, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// URI, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Abstract text, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abstract_note: Option<String>,
}

/// Output from a plugin render invocation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RenderOutput {
    /// "ok" or "error".
    pub status: String,
    /// HTML fragment to embed in the SSR page.
    #[serde(default)]
    pub html: String,
    /// Optional tab label override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Error message when status is "error".
    #[serde(default)]
    pub message: Option<String>,
}

impl RenderOutput {
    /// Create a successful render output with the given HTML.
    pub fn ok(html: impl Into<String>) -> Self {
        Self {
            status: "ok".to_string(),
            html: html.into(),
            label: None,
            message: None,
        }
    }

    /// Create an error render output.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            html: String::new(),
            label: None,
            message: Some(message.into()),
        }
    }

    /// Override the tab label.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// Output from a plugin run invocation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunOutput {
    /// "ok" or "error".
    pub status: String,
    /// Text or HTML result content produced by the action.
    #[serde(default)]
    pub result: Option<String>,
    /// Content type of the result field.
    #[serde(default)]
    pub content_type: Option<String>,
    /// Suggested download filename for result content.
    #[serde(default)]
    pub filename: Option<String>,
    /// Error message when status is "error".
    #[serde(default)]
    pub message: Option<String>,
}

impl RunOutput {
    /// Create a successful run output with the given result text.
    pub fn ok(result: impl Into<String>) -> Self {
        Self {
            status: "ok".to_string(),
            result: Some(result.into()),
            content_type: None,
            filename: None,
            message: None,
        }
    }

    /// Create an error run output.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            result: None,
            content_type: None,
            filename: None,
            message: Some(message.into()),
        }
    }

    /// Set the content type of the result.
    #[must_use]
    pub fn content_type(mut self, ct: impl Into<String>) -> Self {
        self.content_type = Some(ct.into());
        self
    }

    /// Set the suggested download filename of the result.
    #[must_use]
    pub fn filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }
}
