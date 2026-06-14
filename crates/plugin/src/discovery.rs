//! Plugin discovery: scanning plugin directories and reading manifests.

use std::path::{Path, PathBuf};

use crate::manifest::{PluginManifest, PluginUiSpec};

/// A discovered plugin with its identity, optional UI spec, and paths.
#[derive(Clone, Debug)]
pub struct DiscoveredPlugin {
    /// Plugin root directory containing plugin.toml.
    pub dir: PathBuf,
    /// Parsed plugin identity manifest.
    pub manifest: PluginManifest,
    /// Parsed declarative UI spec, when a ui.toml is present and valid.
    pub ui: Option<PluginUiSpec>,
    /// Full path to the plugin executable.
    pub executable: PathBuf,
}

/// Scan the plugins directory and return discovered plugins.
///
/// Each subdirectory that contains a valid `plugin.toml` and a matching
/// executable is registered as a plugin.
#[must_use]
pub fn discover_plugins(plugins_dir: &Path) -> Vec<DiscoveredPlugin> {
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
        })
        .filter_map(|entry| {
            let dir = entry.path();
            let manifest_path = dir.join("plugin.toml");
            let toml_text = std::fs::read_to_string(&manifest_path).ok()?;
            let manifest = PluginManifest::parse(&toml_text).ok()?;
            let executable = manifest
                .executable
                .as_deref()
                .map(|name| dir.join(name))
                .filter(|path| path.is_file())
                .or_else(|| {
                    #[cfg(windows)]
                    {
                        let path = dir.join(format!("{}.exe", manifest.name));
                        path.is_file().then_some(path)
                    }
                    #[cfg(not(windows))]
                    {
                        None
                    }
                })
                .or_else(|| {
                    let path = dir.join(&manifest.name);
                    path.is_file().then_some(path)
                })?;
            let ui_name =
                manifest.ui.clone().unwrap_or_else(|| "ui.toml".to_string());
            let ui = std::fs::read_to_string(dir.join(&ui_name))
                .ok()
                .and_then(|text| match PluginUiSpec::parse(&text) {
                    Ok(spec) => Some(spec),
                    Err(error) => {
                        tracing::warn!(
                            plugin = %manifest.name,
                            %error,
                            "skipping invalid ui.toml; plugin loads without UI"
                        );
                        None
                    }
                });
            Some(DiscoveredPlugin { dir, manifest, ui, executable })
        })
        .collect()
}

impl DiscoveredPlugin {
    /// Return the plugin name from its manifest.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.manifest.name
    }
}

#[cfg(test)]
mod tests {
    use super::discover_plugins;

    #[test]
    fn discovery_loads_optional_ui_spec() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("bibtexer");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"bibtexer\"\nexecutable = \"bibtexer\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("ui.toml"),
            "[[actions]]\nid = \"x\"\nlabel = \"X\"\nmount = \"action_button\"\n",
        )
        .unwrap();
        let exe = if cfg!(windows) { "bibtexer.exe" } else { "bibtexer" };
        std::fs::write(dir.join(exe), b"").unwrap();

        let plugins = discover_plugins(temp.path());
        assert_eq!(plugins.len(), 1);
        let ui = plugins[0].ui.as_ref().expect("ui spec loaded");
        assert_eq!(ui.actions.len(), 1);
    }

    #[test]
    fn discovery_without_ui_spec_is_none() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("plain");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"plain\"\nexecutable = \"plain\"\n",
        )
        .unwrap();
        let exe = if cfg!(windows) { "plain.exe" } else { "plain" };
        std::fs::write(dir.join(exe), b"").unwrap();

        let plugins = discover_plugins(temp.path());
        assert_eq!(plugins.len(), 1);
        assert!(plugins[0].ui.is_none());
    }
}
