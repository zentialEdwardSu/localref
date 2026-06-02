//! HTTP API client boundary for the Localref browser controller.

use serde::Deserialize;

use crate::query::RouteState;

/// Complete UI state returned by the server JSON contract.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct UiStateDto {
    /// Items visible after current browsing filters.
    pub items: Vec<ItemSummaryDto>,
    /// Categories available in the library.
    pub categories: Vec<CategorySummaryDto>,
    /// Recent daemon events.
    pub events: Vec<EventDto>,
    /// Number of pending imports.
    pub pending_count: usize,
    /// Checkbox-selected item ids.
    pub selected_ids: Vec<String>,
    /// Item ids targeted by category controls.
    pub category_target_ids: Vec<String>,
    /// Active detail item id.
    pub active_id: Option<String>,
    /// Active metadata fields for the detail pane.
    pub active_detail: Option<ActiveDetailDto>,
    /// Active right-pane tab.
    pub tab: String,
    /// Return path preserving current server route state.
    pub return_to: String,
    /// Compact daemon status label.
    pub status_label: String,
    /// Files for the active item.
    pub files: Vec<FileEntryDto>,
    /// Current automatic-classification rules text.
    pub rules_text: String,
    /// Optional rules save result.
    pub rules_notice: Option<RulesNoticeDto>,
}

/// Metadata fields for the active detail pane.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ActiveDetailDto {
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

/// Item summary consumed by the WASM renderer.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ItemSummaryDto {
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
}

/// Category summary consumed by filters and editors.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CategorySummaryDto {
    /// Category path.
    pub path: String,
    /// Number of items in the category.
    pub item_count: usize,
}

/// Event summary consumed by the events panel.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct EventDto {
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

/// File entry consumed by the files tab.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FileEntryDto {
    /// Path relative to the item directory.
    pub path: String,
    /// Entry kind label.
    pub kind: String,
    /// File size in bytes, when present.
    pub bytes: Option<u64>,
}

/// Rules save notice consumed by the browser UI.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RulesNoticeDto {
    /// Rules parsed successfully.
    Saved {
        /// Parsed rules.
        rules: Vec<RuleSummaryDto>,
    },
    /// Rules failed to parse or validate.
    Error {
        /// User-visible error message.
        message: String,
    },
}

/// One automatic-classification rule summary.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RuleSummaryDto {
    /// Rule name.
    pub name: String,
    /// Target category path.
    pub target: String,
    /// Query expression.
    pub query: String,
}

/// Return the server JSON state URL for a browser route.
pub fn state_url(route: &RouteState) -> String {
    let query = route.to_query_string();
    if query.is_empty() {
        "/ui/state".to_string()
    } else {
        format!("/ui/state?{query}")
    }
}

/// Fetch and decode UI state from `/ui/state` in the browser.
#[cfg(target_arch = "wasm32")]
pub async fn fetch_state(
    route: &RouteState,
) -> Result<UiStateDto, wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window()
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("missing window"))?;
    let response_value =
        JsFuture::from(window.fetch_with_str(&state_url(route))).await?;
    let response: web_sys::Response = response_value.dyn_into()?;
    if !response.ok() {
        return Err(wasm_bindgen::JsValue::from_str(&format!(
            "state fetch failed: {}",
            response.status()
        )));
    }
    let text_value = JsFuture::from(response.text()?).await?;
    let text = text_value.as_string().ok_or_else(|| {
        wasm_bindgen::JsValue::from_str("state body is not text")
    })?;
    serde_json::from_str(&text).map_err(|error| {
        wasm_bindgen::JsValue::from_str(&format!(
            "state decode failed: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::{UiStateDto, state_url};
    use crate::query::RouteState;

    #[test]
    fn state_url_uses_ui_state_endpoint_and_route_query() {
        let route = RouteState {
            search: Some("alpha".to_string()),
            category: None,
            active_id: Some("lr:zotero:alpha".to_string()),
            selected_ids: Vec::new(),
            tab: "files".to_string(),
        };

        assert_eq!(
            state_url(&route),
            "/ui/state?q=alpha&active=lr%3Azotero%3Aalpha&tab=files"
        );
    }

    #[test]
    fn ui_state_dto_decodes_phase_one_fields() {
        let json = r#"{
            "items": [{"id": "lr:zotero:alpha", "title": "Alpha", "authors": ["Ada"], "item_type": "journalArticle", "categories": ["Inbox"], "main_file": "paper.pdf"}],
            "categories": [{"path": "Inbox", "item_count": 1}],
            "events": [],
            "pending_count": 0,
            "selected_ids": ["lr:zotero:alpha"],
            "category_target_ids": ["lr:zotero:alpha"],
            "active_id": "lr:zotero:alpha",
            "active_detail": {"metadata_revision": "rev1", "title": "Alpha", "authors": "Ada", "item_type": "journalArticle", "year": 2026, "doi": null, "venue": null, "language": "en", "uri": null, "abstract_note": "Abstract"},
            "tab": "files",
            "return_to": "/?active=lr:zotero:alpha&tab=files",
            "status_label": "Running",
            "files": [{"path": "paper.pdf", "kind": "file", "bytes": 12}],
            "rules_text": "[[rules]]\nname = \"Inbox\"\n",
            "rules_notice": null
        }"#;

        let state: UiStateDto = serde_json::from_str(json).unwrap();

        assert_eq!(state.items[0].title, "Alpha");
        assert_eq!(state.categories[0].item_count, 1);
        assert_eq!(state.active_detail.as_ref().unwrap().authors, "Ada");
        assert_eq!(state.selected_ids, vec!["lr:zotero:alpha".to_string()]);
        assert_eq!(state.files[0].path, "paper.pdf");
    }
}
