//! Plugin manifest types and TOML parsing.

use serde::Deserialize;

/// Describes a discovered plugin's capabilities.
#[derive(Clone, Debug, Deserialize)]
pub struct PluginManifest {
    /// Plugin machine-readable name.
    pub name: String,
    /// CLI executable path relative to the plugin directory.
    #[serde(default)]
    pub executable: Option<String>,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Named actions the plugin exposes.
    #[serde(default)]
    pub actions: Vec<ActionSpec>,
    /// SSR pages the plugin provides.
    #[serde(default)]
    pub pages: Vec<PageSpec>,
}

/// One plugin action registered for a mount point.
#[derive(Clone, Debug, Deserialize)]
pub struct ActionSpec {
    /// Action identifier passed to `plugin run <id>`.
    pub id: String,
    /// Display label for buttons and menu items.
    pub label: String,
    /// Where this action appears in the UI.
    pub mount: ActionMount,
}

/// Mount point for plugin actions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ActionMount {
    /// Top-bar control button.
    ActionButton,
    /// Right-click context menu item.
    ContextMenu,
}

/// One SSR page provided by the plugin.
#[derive(Clone, Debug, Deserialize)]
pub struct PageSpec {
    /// Page identifier passed to `plugin render --page <id>`.
    pub id: String,
    /// Tab label displayed in the UI.
    pub label: String,
    /// Where this page is mounted.
    pub mount: PageMount,
    /// URL route segment for this page.
    pub route: String,
}

/// Mount point for plugin pages.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PageMount {
    /// Detail pane tab (next to Metadata, Files, Rules).
    DetailTab,
    /// Inline page mounted on the single-item metadata page.
    MetadataPage,
    /// Inline page mounted on the multi-selection page.
    SelectionPage,
}

impl PluginManifest {
    /// Parse a manifest from its TOML source text.
    ///
    /// # Errors
    ///
    /// Returns an error when the TOML text does not match the plugin manifest
    /// schema.
    pub fn parse(toml_text: &str) -> Result<Self, crate::PluginError> {
        let manifest: Self = toml::from_str(toml_text)
            .map_err(|error| crate::PluginError::Parse(error.to_string()))?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::{PageMount, PluginManifest};

    #[test]
    fn parse_manifest_accepts_cli_and_fixed_page_mounts() {
        let manifest = PluginManifest::parse(
            r#"
name = "cite"
executable = "bin/cite-cli"

[[pages]]
id = "metadata"
label = "Metadata Tools"
mount = "metadata_page"
route = "metadata-tools"

[[pages]]
id = "selection"
label = "Selection Tools"
mount = "selection_page"
route = "selection-tools"
"#,
        )
        .expect("manifest should parse fixed plugin page mount points");

        assert_eq!(manifest.executable.as_deref(), Some("bin/cite-cli"));
        assert_eq!(manifest.pages[0].mount, PageMount::MetadataPage);
        assert_eq!(manifest.pages[1].mount, PageMount::SelectionPage);
    }
}
