//! Plugin state convenience types and functions.

use std::collections::HashMap;
use std::fmt::Write as _;

/// Re-export of the host's plugin UI state.
pub type PluginState = localref_plugin::state::PluginUiState;

/// Form parameters passed to a plugin action.
pub type Params = HashMap<String, String>;

/// Build a `return_to` URL from the current route state.
#[allow(dead_code)]
#[must_use]
pub fn return_to(state: &PluginState) -> String {
    let selected = (!state.selected_ids.is_empty())
        .then(|| format!("selected={}", state.selected_ids.join(",")));
    let parts: Vec<String> = [
        state.search.as_ref().map(|q| format!("q={}", encode_query(q))),
        state
            .category
            .as_ref()
            .map(|cat| format!("category={}", encode_query(cat))),
        selected,
        state
            .active_id
            .as_ref()
            .map(|active| format!("active={}", encode_query(active))),
        Some(format!("tab={}", encode_query(&state.tab))),
    ]
    .into_iter()
    .flatten()
    .collect();
    format!("/?{}", parts.join("&"))
}

/// Selected item ids.
#[allow(dead_code)]
#[must_use]
pub fn selected(state: &PluginState) -> &[String] {
    &state.selected_ids
}

/// Whether a search query is active.
#[allow(dead_code)]
#[must_use]
pub const fn has_search(state: &PluginState) -> bool {
    state.search.is_some()
}

/// Whether a category filter is active.
#[allow(dead_code)]
#[must_use]
pub const fn has_category(state: &PluginState) -> bool {
    state.category.is_some()
}

/// Percent-encode one query parameter value.
fn encode_query(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':' | b',')
        {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
