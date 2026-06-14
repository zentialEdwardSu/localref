//! URL state and view model assembly for the Localref web UI.

use localref_core::logging::LogEntry;
use localref_core::model::{
    Creator, ItemDocument, ItemFileEntry, Metadata, MetadataDocument,
};
use localref_core::rules::{RuleSet, RuleSummary};
use localref_core::storage::CategorySummary;
use localref_core::{DaemonStatus, LocalrefDaemon, PauseMode};
use localref_plugin::discovery::DiscoveredPlugin;

use crate::route::encode_query;
use localref_plugin::manifest::{UiMount, UiTarget};
use serde::Deserialize;

use crate::model::{
    PluginButtonDef, PluginDisplayDef, PluginFieldDef, PluginMenuItemDef,
    PluginPageDef, PluginPreviewDef, PluginTabDef,
};

/// URL query state used by the browser UI.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UiQuery {
    /// Stored q.
    pub(crate) q: Option<String>,
    /// Stored category.
    pub(crate) category: Option<String>,
    /// Stored selected.
    pub(crate) selected: Option<String>,
    /// Stored active.
    pub(crate) active: Option<String>,
    /// Stored tab.
    pub(crate) tab: Option<String>,
    /// Stored rules status.
    pub(crate) rules_status: Option<String>,
    /// Stored rules error.
    pub(crate) rules_error: Option<String>,
    /// Stored plugin.
    pub(crate) plugin: Option<String>,
    /// Stored plugin error.
    pub(crate) plugin_error: Option<String>,
    #[serde(default)]
    /// Stored item.
    pub(crate) item: Vec<String>,
}

/// Server-side model consumed by render components.
#[derive(Clone)]
pub struct UiModel {
    /// Stored query.
    pub(crate) query: UiQuery,
    /// Stored items.
    pub(crate) items: Vec<ItemDocument>,
    /// Stored categories.
    pub(crate) categories: Vec<CategorySummary>,
    /// Stored events.
    pub(crate) events: Vec<LogEntry>,
    /// Stored selected ids.
    pub(crate) selected_ids: Vec<String>,
    /// Stored category target ids.
    pub(crate) category_target_ids: Vec<String>,
    /// Stored active id.
    pub(crate) active_id: Option<String>,
    /// Stored active metadata.
    pub(crate) active_metadata: Option<MetadataDocument>,
    /// Stored files.
    pub(crate) files: Vec<ItemFileEntry>,
    /// Stored rules text.
    pub(crate) rules_text: String,
    /// Stored rules notice.
    pub(crate) rules_notice: Option<RulesNotice>,
    /// Stored tab.
    pub(crate) tab: String,
    /// Stored return to.
    pub(crate) return_to: String,
    /// Stored status.
    pub(crate) status: DaemonStatus,
    /// Stored plugin tabs.
    pub(crate) plugin_tabs: Vec<PluginTabDef>,
    /// Stored plugin buttons.
    pub(crate) plugin_buttons: Vec<PluginButtonDef>,
    /// Stored plugin menu items.
    pub(crate) plugin_menu_items: Vec<PluginMenuItemDef>,
    /// Stored declarative plugin pages for fixed slots.
    pub(crate) plugin_slots: Vec<PluginPageDef>,
    /// Stored active declarative plugin page.
    pub(crate) plugin_active_page: Option<PluginPageDef>,
}

/// Floating feedback shown after saving automatic-classification rules.
#[derive(Clone)]
pub enum RulesNotice {
    /// Rules parsed and were saved.
    Saved(Vec<RuleSummary>),
    /// Rules failed to parse or validate.
    Error(String),
}

impl UiModel {
    /// Load all data needed by the first server-rendered page.
    /// # Errors
    ///
    /// Returns an error when the requested UI operation cannot be completed.
    pub fn load(
        daemon: &LocalrefDaemon,
        mut query: UiQuery,
        plugins: &[DiscoveredPlugin],
    ) -> localref_core::error::Result<Self> {
        let all_items = daemon.list_items()?;
        let items = filtered_items(
            all_items,
            query.q.as_deref(),
            query.category.as_deref(),
        );
        let categories = daemon.list_categories()?;
        let events = daemon.events()?;
        let selected_ids = selected_ids(&query);
        let active_id = query
            .active
            .clone()
            .filter(|id| item_id_is_visible(&items, id))
            .or_else(|| {
                selected_ids
                    .iter()
                    .find(|id| item_id_is_visible(&items, id))
                    .cloned()
            })
            .or_else(|| items.first().map(|item| item.id.clone()));
        if query.selected.is_none() && !query.item.is_empty() {
            query.selected = Some(query.item.join(","));
        }
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

        // Build plugin mount data from discovered plugins.
        let plugin_tabs = build_plugin_tabs(plugins);
        let plugin_buttons = build_plugin_buttons(plugins);
        let plugin_menu_items = build_plugin_menu_items(plugins);

        Ok(Self {
            query,
            items,
            categories,
            events,
            selected_ids,
            category_target_ids,
            active_id,
            active_metadata,
            files,
            rules_text,
            rules_notice,
            tab,
            return_to,
            status,
            plugin_tabs,
            plugin_buttons,
            plugin_menu_items,
            plugin_slots: Vec::new(),
            plugin_active_page: None,
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

/// Internal helper for rules notice.
#[must_use]
pub fn rules_notice(query: &UiQuery, rules_text: &str) -> Option<RulesNotice> {
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

/// Internal helper for filtered items.
#[must_use]
pub fn filtered_items(
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

/// Internal helper for item matches search.
#[must_use]
pub fn item_matches_search(item: &ItemDocument, needle: &str) -> bool {
    item.id.to_ascii_lowercase().contains(needle)
        || item.title.to_ascii_lowercase().contains(needle)
        || item
            .authors
            .iter()
            .any(|author| author.to_ascii_lowercase().contains(needle))
}

/// Return whether an item id is present in the currently visible item list.
fn item_id_is_visible(items: &[ItemDocument], id: &str) -> bool {
    items.iter().any(|item| item.id == id)
}

/// Return item ids that category operations should mutate.
pub fn category_target_ids(
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
pub fn selected_ids(query: &UiQuery) -> Vec<String> {
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
#[must_use]
pub fn return_to(
    query: &UiQuery,
    selected_ids: &[String],
    active_id: Option<&str>,
    tab: &str,
) -> String {
    let selected = (!selected_ids.is_empty())
        .then(|| format!("selected={}", selected_ids.join(",")));
    let parts: Vec<String> = [
        optional_text(query.q.as_deref())
            .map(|q| format!("q={}", encode_query(&q))),
        optional_text(query.category.as_deref())
            .map(|category| format!("category={}", encode_query(&category))),
        selected,
        active_id.map(|active_id| format!("active={}", encode_query(active_id))),
        Some(format!("tab={}", encode_query(tab))),
    ]
    .into_iter()
    .flatten()
    .collect();
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
#[must_use]
pub fn parse_author_names(value: Option<&str>) -> Vec<Creator> {
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
pub fn replace_author_creators(
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

/// Return categories that can be added to the current selection.
#[cfg(test)]
#[must_use]
pub fn available_categories<'a>(
    categories: &'a [CategorySummary],
    current: &[String],
) -> Vec<&'a CategorySummary> {
    categories
        .iter()
        .filter(|category| !current.contains(&category.path))
        .collect()
}

/// Escape raw text for an HTML error page.
pub(crate) fn escape_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Build plugin detail-tab defs from discovered UI specs.
#[must_use]
pub fn build_plugin_tabs(plugins: &[DiscoveredPlugin]) -> Vec<PluginTabDef> {
    plugins
        .iter()
        .flat_map(|plugin| {
            let pages = plugin.ui.as_ref().map(|u| u.pages.as_slice());
            pages.unwrap_or(&[]).iter().filter_map(move |page| {
                (page.mount == UiMount::DetailTab).then(|| PluginTabDef {
                    plugin_name: plugin.name().to_string(),
                    page_id: page.id.clone(),
                    label: page.label.clone(),
                    route: page.route.clone(),
                    tab_key: format!("plugin:{}:{}", plugin.name(), page.id),
                })
            })
        })
        .collect()
}

/// Build plugin action-button defs from discovered UI specs.
#[must_use]
pub fn build_plugin_buttons(
    plugins: &[DiscoveredPlugin],
) -> Vec<PluginButtonDef> {
    plugins
        .iter()
        .flat_map(|plugin| {
            let actions = plugin.ui.as_ref().map(|u| u.actions.as_slice());
            actions.unwrap_or(&[]).iter().filter_map(move |action| {
                (action.mount == UiMount::ActionButton).then(|| {
                    PluginButtonDef {
                        plugin_name: plugin.name().to_string(),
                        action_id: action.id.clone(),
                        label: action.label.clone(),
                    }
                })
            })
        })
        .collect()
}

/// Build plugin context-menu defs from discovered UI specs.
#[must_use]
pub fn build_plugin_menu_items(
    plugins: &[DiscoveredPlugin],
) -> Vec<PluginMenuItemDef> {
    plugins
        .iter()
        .flat_map(|plugin| {
            let actions = plugin.ui.as_ref().map(|u| u.actions.as_slice());
            actions.unwrap_or(&[]).iter().filter_map(move |action| {
                (action.mount == UiMount::ContextMenu).then(|| {
                    PluginMenuItemDef {
                        plugin_name: plugin.name().to_string(),
                        action_id: action.id.clone(),
                        label: action.label.clone(),
                    }
                })
            })
        })
        .collect()
}

/// Stable JSON name for a UI mount.
#[must_use]
pub fn ui_mount_name(mount: UiMount) -> &'static str {
    match mount {
        UiMount::ActionButton => "action_button",
        UiMount::ContextMenu => "context_menu",
        UiMount::DetailTab => "detail_tab",
        UiMount::MetadataPage => "metadata_page",
        UiMount::SelectionPage => "selection_page",
    }
}

/// Stable JSON name for a target mode.
#[must_use]
pub fn ui_target_name(target: UiTarget) -> &'static str {
    match target {
        UiTarget::Selection => "selection",
        UiTarget::Active => "active",
        UiTarget::None => "none",
    }
}

/// Stable JSON name for a field kind.
#[must_use]
pub fn field_kind_name(kind: localref_plugin::manifest::FieldKind) -> &'static str {
    use localref_plugin::manifest::FieldKind;
    match kind {
        FieldKind::Text => "text",
        FieldKind::Textarea => "textarea",
        FieldKind::Number => "number",
        FieldKind::Checkbox => "checkbox",
        FieldKind::Select => "select",
        FieldKind::Radio => "radio",
    }
}

/// Convert a parsed `UiPage` into the serializable `PluginPageDef`.
#[must_use]
pub fn page_def(plugin_name: &str, page: &localref_plugin::manifest::UiPage) -> PluginPageDef {
    PluginPageDef {
        mount: ui_mount_name(page.mount).to_string(),
        plugin_name: plugin_name.to_string(),
        page_id: page.id.clone(),
        label: page.label.clone(),
        action_id: page.action.clone(),
        target: ui_target_name(page.target).to_string(),
        fields: page
            .fields
            .iter()
            .map(|f| PluginFieldDef {
                name: f.name.clone(),
                label: f.label.clone(),
                kind: field_kind_name(f.kind).to_string(),
                options: f.options.clone(),
                default: f.default.clone(),
                required: f.required,
                show_if: f.show_if.clone(),
                enabled_if: f.enabled_if.clone(),
            })
            .collect(),
        displays: page
            .display
            .iter()
            .map(|d| PluginDisplayDef {
                id: d.id.clone(),
                text: d.text.clone(),
            })
            .collect(),
        preview: page.preview.as_ref().map(|p| PluginPreviewDef {
            action: p.action.clone(),
            debounce_ms: p.debounce_ms,
            into: p.into.clone(),
        }),
    }
}

