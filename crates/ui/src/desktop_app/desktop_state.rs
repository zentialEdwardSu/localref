//! REST-backed state transitions for the Dioxus desktop UI.
//!
//! This module keeps the component file focused on layout while these helpers
//! handle REST calls, form mapping, and collection refreshes.

use dioxus::prelude::{Readable, Signal, Writable};
use model::{Event, ItemDocument, Metadata};

use crate::{
    CategorySummary, DashboardSnapshot, PendingImportSummary, RestClient,
};

/// Mutable collection signals refreshed together from REST.
pub(super) struct DesktopCollections<'a> {
    /// Dashboard counters.
    pub snapshot: &'a mut Signal<DashboardSnapshot>,
    /// Indexed item rows.
    pub items: &'a mut Signal<Vec<ItemDocument>>,
    /// Category rows.
    pub categories: &'a mut Signal<Vec<CategorySummary>>,
    /// Pending import rows.
    pub pending: &'a mut Signal<Vec<PendingImportSummary>>,
    /// Event rows.
    pub events: &'a mut Signal<Vec<Event>>,
}

/// User-visible feedback signals.
pub(super) struct FeedbackSignals<'a> {
    /// Error message text.
    pub error: &'a mut Signal<String>,
    /// Success or status message text.
    pub notice: &'a mut Signal<String>,
}

/// Metadata edit-form signals.
pub(super) struct MetadataFormSignals<'a> {
    /// Full metadata currently loaded for editing.
    pub edit_metadata: &'a mut Signal<Option<Metadata>>,
    /// Source revision currently loaded for editing.
    pub edit_revision: &'a mut Signal<String>,
    /// Title field.
    pub title: &'a mut Signal<String>,
    /// Item type field.
    pub item_type: &'a mut Signal<String>,
    /// Abstract field.
    pub abstract_note: &'a mut Signal<String>,
    /// DOI field.
    pub doi: &'a mut Signal<String>,
    /// URI field.
    pub uri: &'a mut Signal<String>,
    /// Year field.
    pub year: &'a mut Signal<String>,
    /// Venue field.
    pub venue: &'a mut Signal<String>,
    /// Language field.
    pub language: &'a mut Signal<String>,
}

/// Category add/remove input signals.
pub(super) struct CategoryInputs<'a> {
    /// Item id input.
    pub item_id: &'a Signal<String>,
    /// Category path input.
    pub category: &'a Signal<String>,
}

/// Category rename/merge input signals.
pub(super) struct CategoryMoveInputs<'a> {
    /// Source category input.
    pub from: &'a Signal<String>,
    /// Destination category input.
    pub to: &'a Signal<String>,
}

/// Category and item collection signals refreshed after category writes.
pub(super) struct CategoryLists<'a> {
    /// Category rows.
    pub categories: &'a mut Signal<Vec<CategorySummary>>,
    /// Item rows.
    pub items: &'a mut Signal<Vec<ItemDocument>>,
}

/// Pending import collection and dashboard signals.
pub(super) struct PendingStateSignals<'a> {
    /// Pending import rows.
    pub pending: &'a mut Signal<Vec<PendingImportSummary>>,
    /// Dashboard counters.
    pub snapshot: &'a mut Signal<DashboardSnapshot>,
}

/// Refresh all REST-backed UI collections.
pub(super) fn refresh_all(
    client: &Signal<RestClient>,
    collections: DesktopCollections<'_>,
    feedback: FeedbackSignals<'_>,
) {
    match load_all(client) {
        Ok(loaded) => {
            collections.items.set(loaded.items);
            collections.categories.set(loaded.categories);
            collections.pending.set(loaded.pending);
            collections.events.set(loaded.events);
            collections.snapshot.set(loaded.snapshot);
            feedback.error.set(String::new());
            feedback.notice.set("Refreshed".to_string());
        }
        Err(message) => feedback.error.set(message),
    }
}

/// Load one metadata document into the edit form.
pub(super) fn load_metadata(
    client: &Signal<RestClient>,
    item_id: &str,
    mut form: MetadataFormSignals<'_>,
    feedback: FeedbackSignals<'_>,
) {
    match client.read().get_metadata(item_id) {
        Ok(document) => {
            form.edit_revision.set(document.metadata_revision);
            set_metadata_fields(&document.metadata, &mut form);
            form.edit_metadata.set(Some(document.metadata));
            feedback.error.set(String::new());
            feedback.notice.set(format!("Loaded {item_id}"));
        }
        Err(message) => feedback.error.set(message),
    }
}

/// Save the currently loaded metadata document through REST.
pub(super) fn save_metadata(
    client: &Signal<RestClient>,
    form: MetadataFormSignals<'_>,
    items: &mut Signal<Vec<ItemDocument>>,
    feedback: FeedbackSignals<'_>,
) {
    let Some(mut metadata) = form.edit_metadata.read().clone() else {
        feedback.error.set("No metadata loaded".to_string());
        return;
    };
    metadata.title = form.title.read().trim().to_string();
    metadata.item_type = form.item_type.read().trim().to_string();
    metadata.abstract_note = optional_text(&form.abstract_note.read());
    metadata.doi = optional_text(&form.doi.read());
    metadata.uri = optional_text(&form.uri.read());
    metadata.year = match optional_text(&form.year.read()) {
        Some(value) => match value.parse::<i32>() {
            Ok(parsed) => Some(parsed),
            Err(parse_error) => {
                feedback.error.set(parse_error.to_string());
                return;
            }
        },
        None => None,
    };
    metadata.venue = optional_text(&form.venue.read());
    metadata.language = optional_text(&form.language.read());

    match client.read().patch_metadata(
        &metadata.id,
        form.edit_revision.read().clone(),
        metadata.clone(),
    ) {
        Ok(item) => {
            replace_item(items, item);
            form.edit_metadata.set(Some(metadata));
            feedback.error.set(String::new());
            feedback.notice.set("Metadata saved".to_string());
        }
        Err(message) => feedback.error.set(message),
    }
}

/// Add a category link for the item named in the form.
pub(super) fn category_add(
    client: &Signal<RestClient>,
    input: CategoryInputs<'_>,
    lists: CategoryLists<'_>,
    feedback: FeedbackSignals<'_>,
) {
    match client.read().add_item_category(
        input.item_id.read().trim(),
        input.category.read().trim(),
    ) {
        Ok(_) => refresh_categories(client, lists, feedback),
        Err(message) => feedback.error.set(message),
    }
}

/// Remove a category link for the item named in the form.
pub(super) fn category_remove(
    client: &Signal<RestClient>,
    input: CategoryInputs<'_>,
    lists: CategoryLists<'_>,
    feedback: FeedbackSignals<'_>,
) {
    match client.read().remove_item_category(
        input.item_id.read().trim(),
        input.category.read().trim(),
    ) {
        Ok(_) => refresh_categories(client, lists, feedback),
        Err(message) => feedback.error.set(message),
    }
}

/// Rename or merge a category path.
pub(super) fn category_move(
    client: &Signal<RestClient>,
    input: CategoryMoveInputs<'_>,
    rename: bool,
    lists: CategoryLists<'_>,
    feedback: FeedbackSignals<'_>,
) {
    let result = if rename {
        client
            .read()
            .rename_category(input.from.read().trim(), input.to.read().trim())
    } else {
        client
            .read()
            .merge_category(input.from.read().trim(), input.to.read().trim())
    };
    match result {
        Ok(_) => refresh_categories(client, lists, feedback),
        Err(message) => feedback.error.set(message),
    }
}

/// Confirm one pending connector import.
pub(super) fn confirm_pending(
    client: &Signal<RestClient>,
    id: u64,
    categories_text: &Signal<String>,
    pending_state: PendingStateSignals<'_>,
    feedback: FeedbackSignals<'_>,
) {
    match client
        .read()
        .confirm_pending_import(id, split_categories(&categories_text.read()))
    {
        Ok(_) => refresh_pending(client, pending_state, feedback),
        Err(message) => feedback.error.set(message),
    }
}

/// Cancel one pending connector import.
pub(super) fn cancel_pending(
    client: &Signal<RestClient>,
    id: u64,
    pending_state: PendingStateSignals<'_>,
    feedback: FeedbackSignals<'_>,
) {
    match client.read().cancel_pending_import(id) {
        Ok(_) => refresh_pending(client, pending_state, feedback),
        Err(message) => feedback.error.set(message),
    }
}

/// Load all UI collections from REST.
fn load_all(
    client: &Signal<RestClient>,
) -> Result<LoadedDesktopState, String> {
    let items = client.read().list_items()?;
    let categories = client.read().list_categories()?;
    let pending = client.read().list_pending_imports()?;
    let events = client.read().list_events()?;
    let snapshot = DashboardSnapshot {
        item_count: items.len(),
        category_count: categories.len(),
        pending_count: pending.len(),
        event_count: events.len(),
    };
    Ok(LoadedDesktopState { snapshot, items, categories, pending, events })
}

/// REST data loaded as one desktop refresh unit.
#[derive(Debug)]
struct LoadedDesktopState {
    snapshot: DashboardSnapshot,
    items: Vec<ItemDocument>,
    categories: Vec<CategorySummary>,
    pending: Vec<PendingImportSummary>,
    events: Vec<Event>,
}

/// Copy metadata values into edit-form signals.
fn set_metadata_fields(
    metadata: &Metadata,
    form: &mut MetadataFormSignals<'_>,
) {
    form.title.set(metadata.title.clone());
    form.item_type.set(metadata.item_type.clone());
    form.abstract_note.set(metadata.abstract_note.clone().unwrap_or_default());
    form.doi.set(metadata.doi.clone().unwrap_or_default());
    form.uri.set(metadata.uri.clone().unwrap_or_default());
    form.year
        .set(metadata.year.map(|value| value.to_string()).unwrap_or_default());
    form.venue.set(metadata.venue.clone().unwrap_or_default());
    form.language.set(metadata.language.clone().unwrap_or_default());
}

/// Refresh category and item lists after a category operation.
fn refresh_categories(
    client: &Signal<RestClient>,
    lists: CategoryLists<'_>,
    feedback: FeedbackSignals<'_>,
) {
    match (client.read().list_categories(), client.read().list_items()) {
        (Ok(next_categories), Ok(next_items)) => {
            lists.categories.set(next_categories);
            lists.items.set(next_items);
            feedback.error.set(String::new());
            feedback.notice.set("Categories updated".to_string());
        }
        (Err(message), _) | (_, Err(message)) => feedback.error.set(message),
    }
}

/// Refresh pending-import state and dashboard counts.
fn refresh_pending(
    client: &Signal<RestClient>,
    pending_state: PendingStateSignals<'_>,
    feedback: FeedbackSignals<'_>,
) {
    match (
        client.read().list_pending_imports(),
        client.read().dashboard_snapshot(),
    ) {
        (Ok(next_pending), Ok(next_snapshot)) => {
            pending_state.pending.set(next_pending);
            pending_state.snapshot.set(next_snapshot);
            feedback.error.set(String::new());
            feedback.notice.set("Pending imports updated".to_string());
        }
        (Err(message), _) | (_, Err(message)) => feedback.error.set(message),
    }
}

/// Replace or insert one item in the local UI list.
fn replace_item(items: &mut Signal<Vec<ItemDocument>>, item: ItemDocument) {
    let mut next = items.read().clone();
    if let Some(existing) = next.iter_mut().find(|entry| entry.id == item.id) {
        *existing = item;
    } else {
        next.push(item);
    }
    next.sort_by(|left, right| left.title.cmp(&right.title));
    items.set(next);
}

/// Convert blank form text into `None`.
fn optional_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

/// Split a comma-separated category input field.
fn split_categories(value: &str) -> Option<Vec<String>> {
    let categories = value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if categories.is_empty() { None } else { Some(categories) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_optional_text_is_none() {
        assert_eq!(optional_text("  "), None);
        assert_eq!(optional_text("RIS"), Some("RIS".to_string()));
    }

    #[test]
    fn split_categories_ignores_empty_parts_so_ui_does_not_create_blank_categories()
     {
        assert_eq!(
            split_categories("Wireless/RIS, Inbox, "),
            Some(vec!["Wireless/RIS".to_string(), "Inbox".to_string()])
        );
    }

    #[test]
    fn blank_pending_categories_leave_rule_suggestions_authoritative() {
        assert_eq!(split_categories(" , "), None);
    }
}
