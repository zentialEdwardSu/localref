//! Plugin discovery: scanning plugin directories and reading manifests.

use std::path::{Path, PathBuf};

use crate::manifest::PluginManifest;

/// A discovered plugin with its manifest and filesystem paths.
#[derive(Clone, Debug)]
pub struct DiscoveredPlugin {
    /// Plugin root directory containing plugin.toml.
    pub dir: PathBuf,
    /// Parsed plugin manifest.
    pub manifest: PluginManifest,
    /// Full path to the plugin executable.
    pub executable: PathBuf,
    /// Plugin static asset directory.
    pub static_dir: PathBuf,
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
            let static_dir = dir.join("static");
            Some(DiscoveredPlugin { dir, manifest, executable, static_dir })
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
    fn discovery_uses_manifest_executable_path() {
        let temp = tempfile::tempdir().unwrap();
        let plugin_dir = temp.path().join("cite");
        let bin_dir = plugin_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
name = "cite"
executable = "bin/cite-cli"
"#,
        )
        .unwrap();
        std::fs::write(bin_dir.join("cite-cli"), b"").unwrap();

        let plugins = discover_plugins(temp.path());

        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].executable, bin_dir.join("cite-cli"));
    }
}
