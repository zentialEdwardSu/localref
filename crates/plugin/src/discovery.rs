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
        .filter_map(|entry| discover_plugin(&entry.path()))
        .collect()
}

/// Try to discover a single plugin in the given directory.
fn discover_plugin(dir: &Path) -> Option<DiscoveredPlugin> {
    let manifest_path = dir.join("plugin.toml");
    let toml_text = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest = PluginManifest::parse(&toml_text).ok()?;
    let executable = find_executable(dir, &manifest)?;
    let static_dir = dir.join("static");
    Some(DiscoveredPlugin {
        dir: dir.to_path_buf(),
        manifest,
        executable,
        static_dir,
    })
}

/// Find the plugin executable by name in the plugin directory.
#[cfg(windows)]
fn find_executable(dir: &Path, manifest: &PluginManifest) -> Option<PathBuf> {
    if let Some(executable) = manifest.executable.as_deref() {
        let path = dir.join(executable);
        if path.is_file() {
            return Some(path);
        }
    }
    let name = &manifest.name;
    let exe_name = format!("{name}.exe");
    let exe_path = dir.join(&exe_name);
    if exe_path.is_file() {
        return Some(exe_path);
    }
    let path = dir.join(name);
    if path.is_file() {
        return Some(path);
    }
    None
}

/// Find the plugin executable by name in the plugin directory.
#[cfg(not(windows))]
fn find_executable(dir: &Path, manifest: &PluginManifest) -> Option<PathBuf> {
    if let Some(executable) = manifest.executable.as_deref() {
        let path = dir.join(executable);
        if path.is_file() {
            return Some(path);
        }
    }
    let name = &manifest.name;
    let path = dir.join(name);
    if path.is_file() {
        return Some(path);
    }
    None
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
