//! `localref init` — bootstrap configuration and install built-in plugins.
//!
//! Sets the library (repository) path in the Localref config, creates the
//! library and plugins directories, and copies the built-in plugin bundles
//! staged next to the executable into the library's plugins directory so they
//! are discovered on the next launch.

use std::path::{Path, PathBuf};

use localref_core::config::LocalrefConfig;

/// Environment override pointing at the staged built-in plugins directory.
///
/// Defaults to `<exe dir>/builtin-plugins`. `build.py` stages bundles into
/// `target/<profile>/builtin-plugins`, which is the exe's own directory in a
/// dev build.
const BUILTIN_PLUGINS_ENV: &str = "LOCALREF_BUILTIN_PLUGINS";

/// Run the `init` subcommand.
///
/// Updates the repository path when `repo` is given, persists the config,
/// ensures the library and plugins directories exist, and installs the
/// built-in plugins. Existing plugin bundles are left untouched unless `force`
/// is set.
///
/// # Errors
///
/// Returns an error when the config cannot be loaded or saved, or when a
/// directory or plugin file cannot be created.
pub fn run_init(repo: Option<PathBuf>, force: bool) -> Result<(), String> {
    let mut config = LocalrefConfig::load()?;
    if let Some(repo) = repo {
        config.set_library_root(absolute(&repo));
    }
    config.save()?;

    create_dir(config.library_root())?;
    create_dir(config.plugins_dir())?;

    println!("config:  {}", config.source_path().display());
    println!("library: {}", config.library_root().display());
    println!("plugins: {}", config.plugins_dir().display());

    match builtin_plugins_dir() {
        Some(staging) => {
            let installed =
                copy_plugins(&staging, config.plugins_dir(), force)?;
            report_installed(&installed);
        }
        None => {
            println!(
                "no built-in plugins staged (set {BUILTIN_PLUGINS_ENV} or run build.py); config is ready"
            );
        }
    }
    Ok(())
}

/// Print a one-line summary of which built-in plugins were installed.
fn report_installed(installed: &[String]) {
    if installed.is_empty() {
        println!("built-in plugins: none installed (already present)");
    } else {
        println!("built-in plugins installed: {}", installed.join(", "));
    }
}

/// Resolve `path` against the current directory without touching the disk.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
}

/// Create `dir` and all parents, mapping IO errors to messages.
fn create_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| {
        format!("failed to create {}: {error}", dir.display())
    })
}

/// Locate the staged built-in plugins directory, if one exists.
fn builtin_plugins_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(BUILTIN_PLUGINS_ENV) {
        let dir = PathBuf::from(dir);
        return dir.is_dir().then_some(dir);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join("builtin-plugins");
    dir.is_dir().then_some(dir)
}

/// Copy every staged plugin bundle from `staging` into `dest`.
///
/// A bundle is any immediate subdirectory of `staging` containing a
/// `plugin.toml`. Without `force`, bundles whose destination already exists are
/// skipped; with `force` the destination is replaced. Returns the names of the
/// plugins that were installed.
///
/// # Errors
///
/// Returns an error when `staging` cannot be read or a file cannot be copied.
fn copy_plugins(
    staging: &Path,
    dest: &Path,
    force: bool,
) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(staging).map_err(|error| {
        format!("failed to read {}: {error}", staging.display())
    })?;
    let mut installed = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let source = entry.path();
        if !source.join("plugin.toml").is_file() {
            continue;
        }
        let name = entry.file_name();
        let target = dest.join(&name);
        if target.exists() {
            if !force {
                continue;
            }
            std::fs::remove_dir_all(&target).map_err(|error| {
                format!("failed to replace {}: {error}", target.display())
            })?;
        }
        copy_dir(&source, &target)?;
        installed.push(name.to_string_lossy().into_owned());
    }
    installed.sort();
    Ok(installed)
}

/// Recursively copy the contents of `source` into `dest`.
fn copy_dir(source: &Path, dest: &Path) -> Result<(), String> {
    create_dir(dest)?;
    let entries = std::fs::read_dir(source).map_err(|error| {
        format!("failed to read {}: {error}", source.display())
    })?;
    for entry in entries.filter_map(Result::ok) {
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| {
            format!("failed to stat {}: {error}", from.display())
        })?;
        if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map(|_| ()).map_err(|error| {
                format!(
                    "failed to copy {} -> {}: {error}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::copy_plugins;
    use std::fs;
    use std::path::Path;

    fn stage_bundle(staging: &Path, name: &str, body: &str) {
        let dir = staging.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("plugin.toml"), format!("name = \"{name}\"\n"))
            .unwrap();
        let exe = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        };
        fs::write(dir.join(exe), body).unwrap();
    }

    #[test]
    fn copies_all_staged_bundles() {
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path().join("staging");
        let dest = temp.path().join("plugins");
        fs::create_dir_all(&dest).unwrap();
        stage_bundle(&staging, "bibtexer", "v1");
        stage_bundle(&staging, "hooklog", "v1");
        // A non-bundle dir (no plugin.toml) is ignored.
        fs::create_dir_all(staging.join("notaplugin")).unwrap();

        let installed = copy_plugins(&staging, &dest, false).unwrap();

        assert_eq!(installed, vec!["bibtexer", "hooklog"]);
        assert!(dest.join("bibtexer").join("plugin.toml").is_file());
        assert!(dest.join("hooklog").join("plugin.toml").is_file());
    }

    #[test]
    fn skips_existing_without_force() {
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path().join("staging");
        let dest = temp.path().join("plugins");
        fs::create_dir_all(&dest).unwrap();
        stage_bundle(&staging, "bibtexer", "new");
        // Pre-existing install with different content.
        let existing = dest.join("bibtexer");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("plugin.toml"), "old").unwrap();

        let installed = copy_plugins(&staging, &dest, false).unwrap();

        assert!(installed.is_empty());
        assert_eq!(
            fs::read_to_string(existing.join("plugin.toml")).unwrap(),
            "old"
        );
    }

    #[test]
    fn overwrites_existing_with_force() {
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path().join("staging");
        let dest = temp.path().join("plugins");
        fs::create_dir_all(&dest).unwrap();
        stage_bundle(&staging, "bibtexer", "new");
        let existing = dest.join("bibtexer");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("plugin.toml"), "old").unwrap();
        // Stale file not present in staging must be gone after force.
        fs::write(existing.join("stale.txt"), "x").unwrap();

        let installed = copy_plugins(&staging, &dest, true).unwrap();

        assert_eq!(installed, vec!["bibtexer"]);
        assert_eq!(
            fs::read_to_string(existing.join("plugin.toml")).unwrap(),
            "name = \"bibtexer\"\n"
        );
        assert!(!existing.join("stale.txt").exists());
    }
}
