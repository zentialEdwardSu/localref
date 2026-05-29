//! URL state and view model assembly for the Localref web UI.

use leptos::prelude::*;
use localref_core::model::{
    Creator, Event, ItemDocument, ItemFileEntry, Metadata, MetadataDocument,
};
use localref_core::rules::{RuleSet, RuleSummary};
use localref_core::storage::CategorySummary;
use localref_core::{DaemonStatus, LocalrefDaemon, PauseMode};
use serde::Deserialize;

/// URL query state used by the browser UI.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct UiQuery {
    pub(crate) q: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) selected: Option<String>,
    pub(crate) active: Option<String>,
    pub(crate) tab: Option<String>,
    pub(crate) rules_status: Option<String>,
    pub(crate) rules_error: Option<String>,
    #[serde(default)]
    pub(crate) item: Vec<String>,
}

/// Server-side model consumed by render components.
#[derive(Clone)]
pub(crate) struct UiModel {
    pub(crate) query: UiQuery,
    pub(crate) items: Vec<ItemDocument>,
    pub(crate) categories: Vec<CategorySummary>,
    pub(crate) events: Vec<Event>,
    pub(crate) pending_count: usize,
    pub(crate) selected_ids: Vec<String>,
    pub(crate) category_target_ids: Vec<String>,
    pub(crate) active_id: Option<String>,
    pub(crate) active_item: Option<ItemDocument>,
    pub(crate) active_metadata: Option<MetadataDocument>,
    pub(crate) files: Vec<ItemFileEntry>,
    pub(crate) rules_text: String,
    pub(crate) rules_notice: Option<RulesNotice>,
    pub(crate) tab: String,
    pub(crate) return_to: String,
    pub(crate) status: DaemonStatus,
}

/// Floating feedback shown after saving automatic-classification rules.
#[derive(Clone)]
pub(crate) enum RulesNotice {
    /// Rules parsed and were saved.
    Saved(Vec<RuleSummary>),
    /// Rules failed to parse or validate.
    Error(String),
}

impl UiModel {
    /// Load all data needed by the first server-rendered page.
    pub(crate) fn load(
        daemon: &LocalrefDaemon,
        mut query: UiQuery,
    ) -> localref_core::error::Result<Self> {
        let all_items = daemon.list_items()?;
        let items = filtered_items(
            all_items,
            query.q.as_deref(),
            query.category.as_deref(),
        );
        let categories = daemon.list_categories()?;
        let events = daemon.events()?;
        let pending_count = daemon.pending_imports().len();
        let selected_ids = selected_ids(&query);
        let active_id = query
            .active
            .clone()
            .or_else(|| selected_ids.first().cloned())
            .or_else(|| items.first().map(|item| item.id.clone()));
        if query.selected.is_none() && !query.item.is_empty() {
            query.selected = Some(query.item.join(","));
        }
        let active_item = active_id
            .as_ref()
            .and_then(|id| items.iter().find(|item| &item.id == id).cloned());
        let active_metadata = match active_id.as_deref() {
            Some(id) => daemon.get_metadata(id)?,
            None => None,
        };
        let files = match active_id.as_deref() {
            Some(id) => daemon
                .item_files(id)?
                .map(|document| document.files)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let rules_text = daemon.read_rules_text()?;
        let rules_notice = rules_notice(&query, &rules_text);
        let tab = query.tab.clone().unwrap_or_else(|| "metadata".to_string());
        let return_to =
            return_to(&query, &selected_ids, active_id.as_deref(), &tab);
        let status = daemon.status();
        let category_target_ids =
            category_target_ids(&selected_ids, active_id.as_deref());
        Ok(Self {
            query,
            items,
            categories,
            events,
            pending_count,
            selected_ids,
            category_target_ids,
            active_id,
            active_item,
            active_metadata,
            files,
            rules_text,
            rules_notice,
            tab,
            return_to,
            status,
        })
    }

    /// Return a compact daemon status label.
    pub(crate) fn status_label(&self) -> String {
        if !self.status.paused_modes.is_empty() {
            return format!("Paused: {:?}", self.status.paused_modes);
        }
        if self.status.running || self.status.queued_tasks > 0 {
            return "Busy".to_string();
        }
        "Running".to_string()
    }

    /// Return whether watcher processing is currently paused.
    pub(crate) fn watcher_paused(&self) -> bool {
        self.status
            .paused_modes
            .iter()
            .any(|mode| matches!(mode, PauseMode::Watcher | PauseMode::All))
    }
}

fn rules_notice(query: &UiQuery, rules_text: &str) -> Option<RulesNotice> {
    if let Some(error) = optional_text(query.rules_error.as_deref()) {
        return Some(RulesNotice::Error(error));
    }
    if query.rules_status.as_deref() != Some("saved") {
        return None;
    }
    Some(match RuleSet::parse(rules_text) {
        Ok(rules) => RulesNotice::Saved(rules.summaries()),
        Err(error) => RulesNotice::Error(error.to_string()),
    })
}

fn filtered_items(
    items: Vec<ItemDocument>,
    q: Option<&str>,
    category: Option<&str>,
) -> Vec<ItemDocument> {
    let needle = optional_text(q).map(|value| value.to_ascii_lowercase());
    let category = optional_text(category);
    items
        .into_iter()
        .filter(|item| {
            let matches_search = needle
                .as_deref()
                .is_none_or(|needle| item_matches_search(item, needle));
            let matches_category =
                category.as_deref().is_none_or(|category| {
                    item.categories.iter().any(|path| path == category)
                });
            matches_search && matches_category
        })
        .collect()
}

fn item_matches_search(item: &ItemDocument, needle: &str) -> bool {
    item.id.to_ascii_lowercase().contains(needle)
        || item.title.to_ascii_lowercase().contains(needle)
        || item
            .authors
            .iter()
            .any(|author| author.to_ascii_lowercase().contains(needle))
}

/// Return item ids that category operations should mutate.
fn category_target_ids(
    selected_ids: &[String],
    active_id: Option<&str>,
) -> Vec<String> {
    if selected_ids.is_empty() {
        active_id.map(ToOwned::to_owned).into_iter().collect()
    } else {
        selected_ids.to_vec()
    }
}

/// Return selected item ids from URL state.
fn selected_ids(query: &UiQuery) -> Vec<String> {
    if !query.item.is_empty() {
        return query.item.clone();
    }
    query
        .selected
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Build a Localref UI URL preserving search, filters, selection, active item.
pub(crate) fn return_to(
    query: &UiQuery,
    selected_ids: &[String],
    active_id: Option<&str>,
    tab: &str,
) -> String {
    let mut parts = Vec::new();
    if let Some(q) = optional_text(query.q.as_deref()) {
        parts.push(format!("q={}", encode_query(&q)));
    }
    if let Some(category) = optional_text(query.category.as_deref()) {
        parts.push(format!("category={}", encode_query(&category)));
    }
    if !selected_ids.is_empty() {
        parts.push(format!("selected={}", selected_ids.join(",")));
    }
    if let Some(active_id) = active_id {
        parts.push(format!("active={}", encode_query(active_id)));
    }
    parts.push(format!("tab={}", encode_query(tab)));
    format!("/?{}", parts.join("&"))
}

/// Sanitize one redirect destination into a local path.
pub(crate) fn return_path(path: &str) -> String {
    if path.starts_with('/') { path.to_string() } else { "/".to_string() }
}

/// Return trimmed nonempty text.
pub(crate) fn optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Parse semicolon-separated author names for metadata editing.
pub(crate) fn parse_author_names(value: Option<&str>) -> Vec<Creator> {
    value
        .unwrap_or_default()
        .split(';')
        .filter_map(|name| optional_text(Some(name)))
        .map(|name| Creator {
            role: "author".to_string(),
            given: None,
            family: None,
            name: Some(name),
        })
        .collect()
}

/// Replace author creators while preserving non-author creators.
pub(crate) fn replace_author_creators(
    metadata: &mut Metadata,
    authors: Vec<Creator>,
) {
    metadata.creators.retain(|creator| creator.role != "author");
    metadata.creators.extend(authors);
}

/// Summarize author creators for a form input.
pub(crate) fn author_summary(metadata: &Metadata) -> String {
    metadata
        .creators
        .iter()
        .filter(|creator| creator.role == "author")
        .filter_map(Creator::display_name)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Return categories common to every selected item.
pub(crate) fn common_categories(
    items: &[ItemDocument],
    ids: &[String],
) -> Vec<String> {
    let mut common: Option<std::collections::BTreeSet<String>> = None;
    for id in ids {
        let Some(item) = items.iter().find(|item| &item.id == id) else {
            continue;
        };
        let categories = item.categories.iter().cloned().collect();
        common = Some(match common {
            Some(current) => {
                current.intersection(&categories).cloned().collect()
            }
            None => categories,
        });
    }
    common.unwrap_or_default().into_iter().collect()
}

/// Return categories that can be added to the current selection.
pub(crate) fn available_categories<'a>(
    categories: &'a [CategorySummary],
    current: &[String],
) -> Vec<&'a CategorySummary> {
    categories
        .iter()
        .filter(|category| !current.contains(&category.path))
        .collect()
}

/// Render one metadata text input.
pub(crate) fn field(
    label: &'static str,
    name: &'static str,
    value: String,
) -> impl IntoView {
    view! {
        <label class="field">
            <span>{label}</span>
            <input name=name value=value/>
        </label>
    }
}

/// Escape raw text for an HTML error page.
pub(crate) fn escape_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn encode_query(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':' | b',')
        {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
