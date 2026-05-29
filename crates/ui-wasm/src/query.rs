//! URL query state for the Localref browser controller.

/// Browser route state mirrored in Localref UI query parameters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteState {
    /// Search text from the library search box.
    pub search: Option<String>,
    /// Selected category filter path.
    pub category: Option<String>,
    /// Active item id shown in the detail pane.
    pub active_id: Option<String>,
    /// Checkbox-selected item ids.
    pub selected_ids: Vec<String>,
    /// Active right-pane tab.
    pub tab: String,
}

impl RouteState {
    /// Build route state from decoded query key/value pairs.
    pub fn from_pairs<'a>(
        pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        let mut state = Self::default();
        for (key, value) in pairs {
            match key {
                "q" => state.search = optional_text(value),
                "category" => state.category = optional_text(value),
                "active" => state.active_id = optional_text(value),
                "selected" => {
                    state.selected_ids = value
                        .split(',')
                        .filter_map(optional_text)
                        .collect::<Vec<_>>();
                }
                "tab" => {
                    if let Some(tab) = optional_text(value) {
                        state.tab = tab;
                    }
                }
                _ => {}
            }
        }
        state
    }

    /// Serialize route state into a URL query string.
    pub fn to_query_string(&self) -> String {
        let mut parts = Vec::new();
        if let Some(search) = self.search.as_deref().and_then(optional_text) {
            parts.push(format!("q={}", encode_query_value(&search)));
        }
        if let Some(category) =
            self.category.as_deref().and_then(optional_text)
        {
            parts.push(format!("category={}", encode_query_value(&category)));
        }
        if !self.selected_ids.is_empty() {
            let selected = self
                .selected_ids
                .iter()
                .map(|id| encode_query_value(id))
                .collect::<Vec<_>>()
                .join(",");
            parts.push(format!("selected={selected}"));
        }
        if let Some(active_id) =
            self.active_id.as_deref().and_then(optional_text)
        {
            parts.push(format!("active={}", encode_query_value(&active_id)));
        }
        parts.push(format!("tab={}", encode_query_value(&self.tab)));
        parts.join("&")
    }
}

impl Default for RouteState {
    fn default() -> Self {
        Self {
            search: None,
            category: None,
            active_id: None,
            selected_ids: Vec::new(),
            tab: "metadata".to_string(),
        }
    }
}

fn optional_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

fn encode_query_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
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
    fn route_state_serializes_browsing_query_parameters() {
        let route = RouteState {
            search: Some("alpha paper".to_string()),
            category: Some("Wireless/RIS".to_string()),
            active_id: Some("lr:zotero:alpha".to_string()),
            selected_ids: vec![
                "lr:zotero:alpha".to_string(),
                "lr:zotero:beta".to_string(),
            ],
            tab: "files".to_string(),
        };

        assert_eq!(
            route.to_query_string(),
            "q=alpha%20paper&category=Wireless%2FRIS&selected=lr%3Azotero%3Aalpha,lr%3Azotero%3Abeta&active=lr%3Azotero%3Aalpha&tab=files"
        );
    }

    #[test]
    fn route_state_omits_empty_parameters() {
        let route = RouteState {
            search: None,
            category: None,
            active_id: None,
            selected_ids: Vec::new(),
            tab: "metadata".to_string(),
        };

        assert_eq!(route.to_query_string(), "tab=metadata");
    }
}
