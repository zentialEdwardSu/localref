//! View-model types shared by Localref desktop entry points.
//!
//! These structures mirror the JSON payloads exposed by the REST API and keep
//! the Dioxus binary independent from daemon internals.

use localref_core::model::Metadata;
pub use localref_core::model::MetadataDocument;
use serde::{Deserialize, Serialize};

/// Snapshot rendered by the simple desktop dashboard.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct DashboardSnapshot {
    /// Indexed item count.
    pub item_count: usize,
    /// Category count.
    pub category_count: usize,
    /// Pending import count.
    pub pending_count: usize,
    /// Recent event count.
    pub event_count: usize,
}

/// Category summary returned by the REST API.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct CategorySummary {
    /// Category path relative to `Cat/`.
    pub path: String,
    /// Item ids linked into this category.
    pub item_ids: Vec<String>,
}

/// Pending import summary returned by the REST API.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PendingImportSummary {
    /// Pending import id.
    pub id: u64,
    /// Item title.
    pub title: String,
    /// Imported item type, when known.
    pub item_type: Option<String>,
    /// Source URI, when known.
    pub uri: Option<String>,
    /// Rule-suggested categories.
    pub suggested_categories: Vec<String>,
}

/// Request body used to patch one metadata document.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct MetadataPatchRequest {
    /// Revision hash observed by the UI before editing.
    pub expected_revision: String,
    /// Complete metadata replacement.
    pub metadata: Metadata,
}

/// Request body used by category add operations.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct CategoryRequest {
    /// Category path relative to `Cat/`.
    pub category: String,
}

/// Daemon queue status returned by `/api/daemon/status`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct DaemonStatus {
    /// Whether a daemon task is currently running.
    pub running: bool,
    /// Number of queued daemon tasks.
    pub queued_tasks: usize,
    /// Recent task records as API JSON.
    #[serde(default)]
    pub recent_tasks: Vec<serde_json::Value>,
    /// Active pause modes.
    #[serde(default)]
    pub paused_modes: Vec<String>,
}

/// Request body used by daemon pause and resume endpoints.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct PauseRequest {
    /// Pause mode to add or remove.
    pub mode: String,
}
