//! Conversion from daemon-backed UI models into shared app state.
//!
//! The browser JSON contract and SSR renderer both use `ui_app::UiState`, so
//! this module is the only place that maps core daemon models into view data.

use crate::state::{RulesNotice, UiModel, author_summary};
use ui_app::{
    ActiveDetail, CategorySummary, EventSummary, FileEntry, ItemSummary,
    RuleSummary, RulesNotice as AppRulesNotice, UiState,
};

/// Convert the server-side UI model into shared Leptos app state.
pub(crate) fn app_state_from_model(
    model: UiModel,
    repo_name: String,
) -> UiState {
    let main_file = model
        .active_metadata
        .as_ref()
        .and_then(|document| document.metadata.files.main.clone());
    UiState {
        repo_name,
        search: model.query.q.clone(),
        category: model.query.category.clone(),
        status_label: model.status_label(),
        watcher_paused: model.watcher_paused(),
        items: model.items.into_iter().map(item_summary).collect(),
        categories: model
            .categories
            .into_iter()
            .map(category_summary)
            .collect(),
        events: model.events.into_iter().map(event_summary).collect(),
        pending_count: model.pending_count,
        selected_ids: model.selected_ids,
        category_target_ids: model.category_target_ids,
        active_detail: model.active_metadata.as_ref().map(active_detail),
        active_id: model.active_id,
        tab: model.tab,
        return_to: model.return_to,
        files: model
            .files
            .into_iter()
            .map(|file| file_entry(file, main_file.as_deref()))
            .collect(),
        rules_text: model.rules_text,
        rules_notice: model.rules_notice.map(rules_notice),
    }
}

/// Convert one metadata document into detail-pane fields.
fn active_detail(
    document: &localref_core::model::MetadataDocument,
) -> ActiveDetail {
    ActiveDetail {
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

/// Convert one core item document into a UI item summary.
fn item_summary(item: localref_core::model::ItemDocument) -> ItemSummary {
    let mut files = Vec::new();
    if let Some(main) = item.main_file.clone() {
        files.push(main);
    }
    files.extend(item.extra_files.clone());
    ItemSummary {
        id: item.id,
        title: item.title,
        authors: item.authors,
        item_type: item.item_type,
        categories: item.categories,
        main_file: item.main_file,
        files,
    }
}

/// Convert one core category summary into a UI category summary.
fn category_summary(
    category: localref_core::storage::CategorySummary,
) -> CategorySummary {
    CategorySummary {
        item_count: category.item_ids.len(),
        path: category.path,
    }
}

/// Convert one core file entry into a UI file entry.
fn file_entry(
    file: localref_core::model::ItemFileEntry,
    main_file: Option<&str>,
) -> FileEntry {
    let is_main = main_file == Some(file.path.as_str());
    FileEntry { path: file.path, kind: file.kind, bytes: file.bytes, is_main }
}

/// Convert one core event into a UI event summary.
fn event_summary(event: localref_core::model::Event) -> EventSummary {
    EventSummary {
        id: event.id,
        kind: format!("{:?}", event.kind),
        message: event.message,
        item_id: event.item_id,
        path: event.path,
    }
}

/// Convert one server-side rules notice into a UI rules notice.
fn rules_notice(notice: RulesNotice) -> AppRulesNotice {
    match notice {
        RulesNotice::Saved(rules) => AppRulesNotice::Saved {
            rules: rules
                .into_iter()
                .map(|rule| RuleSummary {
                    name: rule.name,
                    target: rule.target,
                    query: rule.query,
                })
                .collect(),
        },
        RulesNotice::Error(message) => AppRulesNotice::Error { message },
    }
}
