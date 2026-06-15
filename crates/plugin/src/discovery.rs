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
/// executable is registered as a plugin. Subdirectories that fail to load
/// (unreadable or invalid manifest, missing executable) are logged and skipped
/// rather than aborting discovery.
#[must_use]
pub fn discover_plugins(plugins_dir: &Path) -> Vec<DiscoveredPlugin> {
    let entries = match std::fs::read_dir(plugins_dir) {
        Ok(entries) => entries,
        Err(error) => {
            // A missing plugins dir is normal on a fresh install; note it at
            // debug so `init` / first-run flows stay quiet by default.
            tracing::debug!(
                target: "localref::plugins",
                dir = %plugins_dir.display(),
                %error,
                "no plugins directory; skipping plugin discovery",
            );
            return Vec::new();
        }
    };
    tracing::info!(
        target: "localref::plugins",
        dir = %plugins_dir.display(),
        "discovering plugins",
    );
    let plugins: Vec<DiscoveredPlugin> = entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
        })
        .filter_map(|entry| load_plugin(&entry.path()))
        .collect();
    let names: Vec<&str> =
        plugins.iter().map(DiscoveredPlugin::name).collect();
    tracing::info!(
        target: "localref::plugins",
        dir = %plugins_dir.display(),
        count = plugins.len(),
        plugins = ?names,
        "plugin discovery complete",
    );
    plugins
}

/// Load one plugin directory, logging and returning `None` on any failure.
// Single caller (`discover_plugins`); kept separate so per-plugin failure
// logging stays readable and is unit-tested directly.
#[allow(clippy::single_call_fn)]
fn load_plugin(dir: &Path) -> Option<DiscoveredPlugin> {
    let manifest_path = dir.join("plugin.toml");
    let toml_text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!(
                target: "localref::plugins",
                dir = %dir.display(),
                %error,
                "skipping directory: cannot read plugin.toml",
            );
            return None;
        }
    };
    let manifest = match PluginManifest::parse(&toml_text) {
        Ok(manifest) => manifest,
        Err(error) => {
            tracing::warn!(
                target: "localref::plugins",
                dir = %dir.display(),
                %error,
                "skipping plugin: invalid plugin.toml",
            );
            return None;
        }
    };
    let Some(executable) = resolve_executable(dir, &manifest) else {
        tracing::warn!(
            target: "localref::plugins",
            plugin = %manifest.name,
            dir = %dir.display(),
            "skipping plugin: no matching executable found",
        );
        return None;
    };
    let ui = load_ui_spec(dir, &manifest);
    tracing::debug!(
        target: "localref::plugins",
        plugin = %manifest.name,
        executable = %executable.display(),
        has_ui = ui.is_some(),
        hooks = manifest.hooks.len(),
        cron = manifest.cron.len(),
        "loaded plugin",
    );
    Some(DiscoveredPlugin { dir: dir.to_path_buf(), manifest, ui, executable })
}

/// Resolve the plugin executable path from the manifest, with name fallbacks.
// Single caller (`load_plugin`); kept separate for clarity of the fallback
// chain.
#[allow(clippy::single_call_fn)]
fn resolve_executable(dir: &Path, manifest: &PluginManifest) -> Option<PathBuf> {
    manifest
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
        })
}

/// Load and parse the optional declarative UI spec, logging parse failures.
// Single caller (`load_plugin`); kept separate for direct testing.
#[allow(clippy::single_call_fn)]
fn load_ui_spec(dir: &Path, manifest: &PluginManifest) -> Option<PluginUiSpec> {
    let ui_name = manifest.ui.as_deref().unwrap_or("ui.toml");
    std::fs::read_to_string(dir.join(ui_name)).ok().and_then(|text| {
        match PluginUiSpec::parse(&text) {
            Ok(spec) => Some(spec),
            Err(error) => {
                tracing::warn!(
                    target: "localref::plugins",
                    plugin = %manifest.name,
                    ui_file = %ui_name,
                    %error,
                    "skipping invalid ui spec; plugin loads without UI"
                );
                None
            }
        }
    })
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
