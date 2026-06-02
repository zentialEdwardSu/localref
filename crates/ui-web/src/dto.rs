//! JSON data contract for the browser-side Localref UI controller.
//!
//! The DTOs in this module are intentionally view-specific. They keep the
//! browser contract stable without exposing every field from the core daemon
//! models.

use serde::Serialize;

use crate::state::{RulesNotice, UiModel, author_summary};

/// Complete UI state returned by `/ui/state`.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct UiStateDto {
    /// Items visible after applying the current search and category filters.
    pub(crate) items: Vec<ItemSummaryDto>,
    /// Categories available in the current library.
    pub(crate) categories: Vec<CategorySummaryDto>,
    /// Recent daemon events for the events panel.
    pub(crate) events: Vec<EventDto>,
    /// Number of imports waiting for user confirmation.
    pub(crate) pending_count: usize,
    /// Item ids selected by checkbox state.
    pub(crate) selected_ids: Vec<String>,
    /// Item ids targeted by category actions.
    pub(crate) category_target_ids: Vec<String>,
    /// Active item id shown in the detail pane.
    pub(crate) active_id: Option<String>,
    /// Active item metadata fields shown in the detail pane.
    pub(crate) active_detail: Option<ActiveDetailDto>,
    /// Active tab name.
    pub(crate) tab: String,
    /// Server-generated return URL preserving current route state.
    pub(crate) return_to: String,
    /// Compact daemon status label.
    pub(crate) status_label: String,
    /// Files shown for the active item.
    pub(crate) files: Vec<FileEntryDto>,
    /// Current automatic-classification rules text.
    pub(crate) rules_text: String,
    /// Optional rules save result notice.
    pub(crate) rules_notice: Option<RulesNoticeDto>,
}

impl UiStateDto {
    /// Convert the server-side UI model into the browser JSON contract.
    pub(crate) fn from_model(model: UiModel) -> Self {
        Self {
            status_label: model.status_label(),
            items: model.items.into_iter().map(ItemSummaryDto::from).collect(),
            categories: model
                .categories
                .into_iter()
                .map(CategorySummaryDto::from)
                .collect(),
            events: model.events.into_iter().map(EventDto::from).collect(),
            pending_count: model.pending_count,
            selected_ids: model.selected_ids,
            category_target_ids: model.category_target_ids,
            active_detail: model
                .active_metadata
                .as_ref()
                .map(ActiveDetailDto::from_metadata),
            active_id: model.active_id,
            tab: model.tab,
            return_to: model.return_to,
            files: model.files.into_iter().map(FileEntryDto::from).collect(),
            rules_text: model.rules_text,
            rules_notice: model.rules_notice.map(RulesNoticeDto::from),
        }
    }
}

/// Metadata fields shown for the active item detail form.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ActiveDetailDto {
    /// Metadata revision used for optimistic save checks.
    pub(crate) metadata_revision: String,
    /// User-visible title.
    pub(crate) title: String,
    /// Semicolon-separated author summary.
    pub(crate) authors: String,
    /// Item type label.
    pub(crate) item_type: String,
    /// Publication year, when known.
    pub(crate) year: Option<i32>,
    /// DOI, when known.
    pub(crate) doi: Option<String>,
    /// Venue, when known.
    pub(crate) venue: Option<String>,
    /// Language, when known.
    pub(crate) language: Option<String>,
    /// URI, when known.
    pub(crate) uri: Option<String>,
    /// Abstract text, when known.
    pub(crate) abstract_note: Option<String>,
}

impl ActiveDetailDto {
    /// Convert one metadata document into detail-pane JSON fields.
    fn from_metadata(
        document: &localref_core::model::MetadataDocument,
    ) -> Self {
        Self {
            metadata_revision: document.metadata_revision.clone(),
            title: document.metadata.title.clone(),
            authors: author_summary(&document.metadata),
            item_type: document.metadata.item_type.clone(),
            year: document.metadata.year,
            doi: document.metadata.doi.clone(),
            venue: document.metadata.venue.clone(),
            language: document.metadata.language.clone(),
            uri: document.metadata.uri.clone(),
            abstract_note: document.metadata.abstract_note.clone(),
        }
    }
}

/// Summary of one library item for sidebar and selection rendering.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ItemSummaryDto {
    /// Stable Localref item id.
    pub(crate) id: String,
    /// User-visible item title.
    pub(crate) title: String,
    /// User-visible author list.
    pub(crate) authors: Vec<String>,
    /// Item type label.
    pub(crate) item_type: String,
    /// Category paths assigned to the item.
    pub(crate) categories: Vec<String>,
    /// Main file path, when present.
    pub(crate) main_file: Option<String>,
}

impl From<localref_core::model::ItemDocument> for ItemSummaryDto {
    fn from(item: localref_core::model::ItemDocument) -> Self {
        Self {
            id: item.id,
            title: item.title,
            authors: item.authors,
            item_type: item.item_type,
            categories: item.categories,
            main_file: item.main_file,
        }
    }
}

/// Category summary for filters and category editors.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct CategorySummaryDto {
    /// Category path relative to the `Cat` root.
    pub(crate) path: String,
    /// Number of items linked into this category.
    pub(crate) item_count: usize,
}

impl From<localref_core::storage::CategorySummary> for CategorySummaryDto {
    fn from(category: localref_core::storage::CategorySummary) -> Self {
        Self { item_count: category.item_ids.len(), path: category.path }
    }
}

/// File entry shown on the active item's files tab.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct FileEntryDto {
    /// Path relative to the active item directory.
    pub(crate) path: String,
    /// Entry kind: file, directory, or other.
    pub(crate) kind: String,
    /// File size in bytes, when available.
    pub(crate) bytes: Option<u64>,
}

impl From<localref_core::model::ItemFileEntry> for FileEntryDto {
    fn from(file: localref_core::model::ItemFileEntry) -> Self {
        Self { path: file.path, kind: file.kind, bytes: file.bytes }
    }
}

/// Daemon event shown in the events panel.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct EventDto {
    /// Monotonic event id.
    pub(crate) id: u64,
    /// Event kind as a display/debug label.
    pub(crate) kind: String,
    /// Human-readable event message.
    pub(crate) message: String,
    /// Related item id, when known.
    pub(crate) item_id: Option<String>,
    /// Related library path, when known.
    pub(crate) path: Option<String>,
}

impl From<localref_core::model::Event> for EventDto {
    fn from(event: localref_core::model::Event) -> Self {
        Self {
            id: event.id,
            kind: format!("{:?}", event.kind),
            message: event.message,
            item_id: event.item_id,
            path: event.path,
        }
    }
}

/// Rules save notice represented in JSON state.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RulesNoticeDto {
    /// Rules parsed successfully after save.
    Saved {
        /// Parsed rule summaries.
        rules: Vec<RuleSummaryDto>,
    },
    /// Rules failed to parse or validate.
    Error {
        /// User-visible validation message.
        message: String,
    },
}

impl From<RulesNotice> for RulesNoticeDto {
    fn from(notice: RulesNotice) -> Self {
        match notice {
            RulesNotice::Saved(rules) => Self::Saved {
                rules: rules
                    .into_iter()
                    .map(|rule| RuleSummaryDto {
                        name: rule.name,
                        target: rule.target,
                        query: rule.query,
                    })
                    .collect(),
            },
            RulesNotice::Error(message) => Self::Error { message },
        }
    }
}

/// One parsed automatic-classification rule summary.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuleSummaryDto {
    /// Rule name.
    pub(crate) name: String,
    /// Target category path.
    pub(crate) target: String,
    /// Rule query expression.
    pub(crate) query: String,
}
