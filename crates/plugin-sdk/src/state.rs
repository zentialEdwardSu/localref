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
    let mut parts = Vec::new();
    if let Some(ref q) = state.search {
        parts.push(format!("q={}", encode_query(q)));
    }
    if let Some(ref cat) = state.category {
        parts.push(format!("category={}", encode_query(cat)));
    }
    if !state.selected_ids.is_empty() {
        parts.push(format!("selected={}", state.selected_ids.join(",")));
    }
    if let Some(ref active) = state.active_id {
        parts.push(format!("active={}", encode_query(active)));
    }
    parts.push(format!("tab={}", encode_query(&state.tab)));
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
