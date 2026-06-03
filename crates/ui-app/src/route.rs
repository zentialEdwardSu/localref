//! URL route state shared by Localref SSR and hydration.

use crate::UiState;

/// Browser route state represented by Localref's query string.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RouteState {
    /// Search query.
    pub search: Option<String>,
    /// Category filter.
    pub category: Option<String>,
    /// Active detail item id.
    pub active_id: Option<String>,
    /// Selected item ids.
    pub selected_ids: Vec<String>,
    /// Active right-pane tab.
    pub tab: String,
    /// Active plugin name.
    pub plugin: Option<String>,
}

impl RouteState {
    /// Build route state from a rendered UI state.
    pub fn from_ui_state(state: &UiState) -> Self {
        Self {
            search: state.search.clone(),
            category: state.category.clone(),
            active_id: state.active_id.clone(),
            selected_ids: state.selected_ids.clone(),
            tab: state.tab.clone(),
            plugin: state.plugin_tabs.first().map(|t| t.plugin_name.clone()),
        }
    }

    /// Build route state from decoded query key-value pairs.
    pub fn from_pairs<'a>(
        pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        let mut route =
            Self { tab: "metadata".to_string(), ..Self::default() };
        for (key, value) in pairs {
            match key {
                "q" => route.search = optional_text(value),
                "category" => route.category = optional_text(value),
                "active" => route.active_id = optional_text(value),
                "selected" => {
                    route.selected_ids =
                        value.split(',').filter_map(optional_text).collect();
                }
                "tab" => {
                    if let Some(tab) = optional_text(value) {
                        route.tab = tab;
                    }
                }
                "plugin" => route.plugin = optional_text(value),
                _ => {}
            }
        }
        route
    }

    /// Serialize route state into a URL query string.
    pub fn to_query_string(&self) -> String {
        let mut parts = Vec::new();
        if let Some(value) = self.search.as_deref().and_then(optional_text) {
            parts.push(format!("q={}", encode_query(&value)));
        }
        if let Some(value) = self.category.as_deref().and_then(optional_text) {
            parts.push(format!("category={}", encode_query(&value)));
        }
        if !self.selected_ids.is_empty() {
            parts.push(format!("selected={}", self.selected_ids.join(",")));
        }
        if let Some(value) = self.active_id.as_deref().and_then(optional_text)
        {
            parts.push(format!("active={}", encode_query(&value)));
        }
        if self.tab != "metadata" {
            parts.push(format!("tab={}", encode_query(&self.tab)));
        }
        if let Some(plugin) = self.plugin.as_deref() {
            parts.push(format!("plugin={}", encode_query(plugin)));
        }
        parts.join("&")
    }

    /// Return the application-local URL for this route.
    pub fn to_path(&self) -> String {
        let query = self.to_query_string();
        if query.is_empty() { "/".to_string() } else { format!("/?{query}") }
    }
}

/// Return the server JSON state URL for a browser route.
#[cfg(feature = "hydrate")]
pub fn state_url(route: &RouteState) -> String {
    let query = route.to_query_string();
    if query.is_empty() {
        "/ui/state".to_string()
    } else {
        format!("/ui/state?{query}")
    }
}

/// Return trimmed nonempty text.
pub fn optional_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
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

#[cfg(test)]
mod tests {
    use super::RouteState;

    #[test]
    fn route_state_omits_empty_parameters() {
        let route = RouteState {
            search: None,
            category: None,
            active_id: None,
            selected_ids: Vec::new(),
            tab: "metadata".to_string(),
            plugin: None,
        };

        assert_eq!(route.to_query_string(), "");
        assert_eq!(route.to_path(), "/");
    }

    #[test]
    fn route_state_serializes_browsing_query_parameters() {
        let route = RouteState {
            search: Some("alpha beta".to_string()),
            category: Some("Wireless/RIS".to_string()),
            active_id: Some("lr:zotero:abc".to_string()),
            selected_ids: vec!["lr:zotero:def".to_string()],
            tab: "files".to_string(),
            plugin: None,
        };

        assert_eq!(
            route.to_query_string(),
            "q=alpha%20beta&category=Wireless%2FRIS&selected=lr:zotero:def&active=lr:zotero:abc&tab=files"
        );
    }
}
