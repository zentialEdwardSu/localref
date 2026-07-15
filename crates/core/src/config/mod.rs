//! Configuration loading for Localref entry points.
//!
//! This crate is the single place that reads Localref process configuration.
//! `LOCALREF_CONFIG` selects the TOML file. When it is not set, Localref reads
//! `~/.localref/config.toml`. Missing files are created with documented
//! defaults; malformed files or invalid values fail loudly.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Environment variable that points at the Localref configuration file.
pub const LOCALREF_CONFIG_ENV: &str = "LOCALREF_CONFIG";

/// Default connector-compatible HTTP bind address.
pub const DEFAULT_CSC_ADDR: &str = "127.0.0.1:23119";

/// Default user-facing REST HTTP bind address.
pub const DEFAULT_REST_ADDR: &str = "127.0.0.1:24817";

/// Default user-facing REST endpoint used by desktop clients.
pub const DEFAULT_REST_ENDPOINT: &str = "http://127.0.0.1:24817";

/// Default maximum size of each daemon log file (10 MiB).
pub const DEFAULT_LOG_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// Default number of rotated daemon log files retained.
pub const DEFAULT_LOG_BACKUP_COUNT: u32 = 2;

/// Runtime configuration shared by Localref binaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalrefConfig {
    /// Stored source path.
    source_path: PathBuf,
    /// Stored workspace display name.
    workspace_name: String,
    /// Stored library root.
    library_root: PathBuf,
    /// Stored rest addr.
    rest_addr: SocketAddr,
    /// Stored rest endpoint.
    rest_endpoint: String,
    /// Stored csc addr.
    csc_addr: SocketAddr,
    /// Stored desktop start hidden.
    desktop_start_hidden: bool,
    /// Stored desktop quiet start.
    desktop_quiet_start: bool,
    /// Stored desktop `DataGrid` column visibility.
    desktop_visible_columns: Vec<String>,
    /// Stored desktop detail panel width in logical pixels.
    desktop_detail_width: u32,
    /// Stored maximum size of each daemon log file.
    log_max_file_bytes: u64,
    /// Stored number of rotated daemon log files retained.
    log_backup_count: u32,
    /// Stored plugins dir.
    plugins_dir: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
/// Internal representation for config file.
struct ConfigFile {
    /// Stored workspace display name. `repo_name` is accepted for migration.
    #[serde(alias = "repo_name")]
    workspace_name: Option<String>,
    /// Stored library root.
    library_root: Option<PathBuf>,
    /// Stored rest.
    rest: Option<RestConfigFile>,
    /// Stored csc.
    csc: Option<CscConfigFile>,
    /// Stored desktop.
    desktop: Option<DesktopConfigFile>,
    /// Stored logging settings.
    logging: Option<LoggingConfigFile>,
}

#[derive(Debug, Default, Deserialize)]
/// Internal representation for rest config file.
struct RestConfigFile {
    /// Stored addr.
    addr: Option<String>,
    /// Stored endpoint.
    endpoint: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
/// Internal representation for csc config file.
struct CscConfigFile {
    /// Stored addr.
    addr: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
/// Internal representation for desktop config file.
struct DesktopConfigFile {
    /// Stored start hidden.
    start_hidden: Option<bool>,
    /// Stored quiet start.
    quiet_start: Option<bool>,
    /// Stored visible optional `DataGrid` columns.
    visible_columns: Option<Vec<String>>,
    /// Stored detail panel width in logical pixels.
    detail_width: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
/// Internal representation for daemon logging configuration.
struct LoggingConfigFile {
    /// Maximum size of each active or rotated log file.
    max_file_bytes: Option<u64>,
    /// Number of rotated files retained after the active log.
    backup_count: Option<u32>,
}

impl LocalrefConfig {
    /// Load configuration from `LOCALREF_CONFIG` or the default path.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn load() -> Result<Self, String> {
        let path = config_path()?;
        Self::load_from_path(path)
    }

    /// Load configuration from one explicit TOML file path.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn load_from_path(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        if !path.exists() {
            let file = ConfigFile::default();
            let config = Self::from_config_file(path, file)?;
            config.save()?;
            return Ok(config);
        }
        let text = std::fs::read_to_string(&path).map_err(|error| {
            format!("failed to read {}: {error}", path.display())
        })?;
        let file = toml::from_str::<ConfigFile>(&text).map_err(|error| {
            format!("failed to parse {}: {error}", path.display())
        })?;
        Self::from_config_file(path, file)
    }

    /// Serialize this configuration to the on-disk TOML representation.
    #[must_use]
    fn to_toml(&self) -> String {
        let workspace_name = escape_toml_basic(&self.workspace_name);
        // A TOML literal (single-quoted) string cannot contain an apostrophe at
        // all, so emit the path as a basic (double-quoted) string with the
        // backslashes and quotes escaped. This round-trips Windows paths and
        // usernames containing `'` (e.g. C:\Users\O'Brien\libroot).
        let library_root =
            escape_toml_basic(&self.library_root.to_string_lossy());
        let visible_columns = self
            .desktop_visible_columns
            .iter()
            .map(|column| format!("\"{}\"", escape_toml_basic(column)))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "workspace_name = \"{workspace_name}\"\nlibrary_root = \"{library_root}\"\n\n[rest]\naddr = \"{}\"\nendpoint = \"{}\"\n\n[csc]\naddr = \"{}\"\n\n[desktop]\nstart_hidden = {}\nquiet_start = {}\nvisible_columns = [{visible_columns}]\ndetail_width = {}\n\n[logging]\nmax_file_bytes = {}\nbackup_count = {}\n",
            self.rest_addr,
            self.rest_endpoint,
            self.csc_addr,
            self.desktop_start_hidden,
            self.desktop_quiet_start,
            self.desktop_detail_width,
            self.log_max_file_bytes,
            self.log_backup_count
        )
    }

    /// Write this configuration to its `source_path`, creating parent dirs.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent directory or file cannot be written.
    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.source_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("failed to create {}: {error}", parent.display())
            })?;
        }
        std::fs::write(&self.source_path, self.to_toml()).map_err(|error| {
            format!("failed to write {}: {error}", self.source_path.display())
        })
    }

    /// Point the library root at `root`, re-deriving the plugins directory.
    pub fn set_library_root(&mut self, root: impl Into<PathBuf>) {
        let root = root.into();
        self.plugins_dir = root.join(".localref").join("plugins");
        self.library_root = root;
    }

    /// Change the workspace display name.
    ///
    /// # Errors
    ///
    /// Returns an error when `name` is empty after trimming.
    pub fn set_workspace_name(
        &mut self,
        name: impl Into<String>,
    ) -> Result<(), String> {
        let name = name.into().trim().to_string();
        if name.is_empty() {
            return Err("workspace_name must not be empty".to_string());
        }
        self.workspace_name = name;
        Ok(())
    }

    /// Backward-compatible alias for callers that still use repository naming.
    ///
    /// # Errors
    ///
    /// Returns an error when `name` is empty after trimming.
    pub fn set_repo_name(
        &mut self,
        name: impl Into<String>,
    ) -> Result<(), String> {
        self.set_workspace_name(name)
    }

    /// Change the REST bind address after validating it.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not a socket address.
    pub fn set_rest_addr(&mut self, value: &str) -> Result<(), String> {
        self.rest_addr = parse_addr(value, "rest.addr")?;
        Ok(())
    }

    /// Change the public REST endpoint used by plugins.
    ///
    /// # Errors
    ///
    /// Returns an error when the endpoint does not use HTTP or HTTPS.
    pub fn set_rest_endpoint(
        &mut self,
        value: impl Into<String>,
    ) -> Result<(), String> {
        let value = value.into().trim().trim_end_matches('/').to_string();
        if !(value.starts_with("http://") || value.starts_with("https://")) {
            return Err("rest.endpoint must start with http:// or https://"
                .to_string());
        }
        self.rest_endpoint = value;
        Ok(())
    }

    /// Change the Zotero Connector-compatible bind address.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not a socket address.
    pub fn set_csc_addr(&mut self, value: &str) -> Result<(), String> {
        self.csc_addr = parse_addr(value, "csc.addr")?;
        Ok(())
    }

    /// Return the file that supplied this configuration.
    #[must_use]
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// Return the configured Localref library root.
    #[must_use]
    pub fn library_root(&self) -> &Path {
        &self.library_root
    }

    /// Return the configured workspace display name.
    #[must_use]
    pub fn workspace_name(&self) -> &str {
        &self.workspace_name
    }

    /// Backward-compatible alias for the workspace display name.
    #[must_use]
    pub fn repo_name(&self) -> &str {
        self.workspace_name()
    }

    /// Return the REST API bind address for server binaries.
    #[must_use]
    pub fn rest_addr(&self) -> SocketAddr {
        self.rest_addr
    }

    /// Return the REST API endpoint for desktop clients.
    #[must_use]
    pub fn rest_endpoint(&self) -> &str {
        &self.rest_endpoint
    }

    /// Return the Zotero Connector-compatible bind address.
    #[must_use]
    pub fn csc_addr(&self) -> SocketAddr {
        self.csc_addr
    }

    /// Return whether tray-hosted startup should skip the initial window.
    #[must_use]
    pub fn desktop_start_hidden(&self) -> bool {
        self.desktop_start_hidden
    }

    /// Return whether tray-hosted startup should avoid console chatter.
    #[must_use]
    pub fn desktop_quiet_start(&self) -> bool {
        self.desktop_quiet_start
    }

    /// Change whether the desktop app starts without showing its main window.
    pub fn set_desktop_start_hidden(&mut self, value: bool) {
        self.desktop_start_hidden = value;
    }

    /// Change whether desktop/headless startup suppresses nonessential output.
    pub fn set_desktop_quiet_start(&mut self, value: bool) {
        self.desktop_quiet_start = value;
    }

    /// Return the optional `DataGrid` columns visible in the desktop library.
    #[must_use]
    pub fn desktop_visible_columns(&self) -> &[String] {
        &self.desktop_visible_columns
    }

    /// Replace the optional `DataGrid` columns visible in the desktop library.
    pub fn set_desktop_visible_columns(&mut self, mut columns: Vec<String>) {
        const KNOWN_COLUMNS: [&str; 5] =
            ["Author", "Venue", "Year", "Type", "Categories"];
        columns.retain(|column| KNOWN_COLUMNS.contains(&column.as_str()));
        self.desktop_visible_columns = KNOWN_COLUMNS
            .into_iter()
            .filter(|known| columns.iter().any(|column| column == known))
            .map(str::to_string)
            .collect();
    }

    /// Return the desktop detail panel width in logical pixels.
    #[must_use]
    pub fn desktop_detail_width(&self) -> u32 {
        self.desktop_detail_width
    }

    /// Change the desktop detail panel width, constrained to a usable range.
    pub fn set_desktop_detail_width(&mut self, width: u32) {
        self.desktop_detail_width = width.clamp(340, 620);
    }

    /// Return the maximum size of each daemon log file.
    #[must_use]
    pub fn log_max_file_bytes(&self) -> u64 {
        self.log_max_file_bytes
    }

    /// Return the number of rotated daemon log files retained.
    #[must_use]
    pub fn log_backup_count(&self) -> u32 {
        self.log_backup_count
    }

    /// Return the directory where plugins are discovered.
    #[must_use]
    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    /// Internal helper for from config file.
    fn from_config_file(
        source_path: PathBuf,
        file: ConfigFile,
    ) -> Result<Self, String> {
        let library_root = match file.library_root {
            Some(path) => path,
            None => home_dir()?.join(".localref").join("libroot"),
        };
        let workspace_name = file
            .workspace_name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Localref".to_string());
        let rest = file.rest.unwrap_or_default();
        let csc = file.csc.unwrap_or_default();
        let desktop = file.desktop.unwrap_or_default();
        let logging = file.logging.unwrap_or_default();
        let visible_columns = desktop.visible_columns.unwrap_or_else(|| {
            ["Author", "Venue", "Year", "Type"]
                .into_iter()
                .map(str::to_string)
                .collect()
        });
        let rest_addr = parse_addr(
            rest.addr.as_deref().unwrap_or(DEFAULT_REST_ADDR),
            "rest.addr",
        )?;
        let rest_endpoint =
            rest.endpoint.unwrap_or_else(|| format!("http://{rest_addr}"));
        let csc_addr = parse_addr(
            csc.addr.as_deref().unwrap_or(DEFAULT_CSC_ADDR),
            "csc.addr",
        )?;
        let plugins_dir = library_root.join(".localref").join("plugins");
        let log_max_file_bytes =
            logging.max_file_bytes.unwrap_or(DEFAULT_LOG_MAX_FILE_BYTES);
        if log_max_file_bytes < 64 * 1024 {
            return Err(
                "logging.max_file_bytes must be at least 65536".to_string()
            );
        }
        let log_backup_count =
            logging.backup_count.unwrap_or(DEFAULT_LOG_BACKUP_COUNT);
        if log_backup_count > 10 {
            return Err(
                "logging.backup_count must be between 0 and 10".to_string()
            );
        }
        let mut config = Self {
            source_path,
            workspace_name,
            library_root,
            rest_addr,
            rest_endpoint,
            csc_addr,
            desktop_start_hidden: desktop.start_hidden.unwrap_or(true),
            desktop_quiet_start: desktop.quiet_start.unwrap_or(true),
            desktop_visible_columns: Vec::new(),
            desktop_detail_width: desktop.detail_width.unwrap_or(420),
            log_max_file_bytes,
            log_backup_count,
            plugins_dir,
        };
        config.set_desktop_visible_columns(visible_columns);
        let detail_width = config.desktop_detail_width;
        config.set_desktop_detail_width(detail_width);
        Ok(config)
    }
}

/// Return the configured TOML path from `LOCALREF_CONFIG` or `~/.localref`.
/// # Errors
///
/// Returns an error when the operation cannot be completed.
pub fn config_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(LOCALREF_CONFIG_ENV) {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".localref").join("config.toml"))
}

/// Internal helper for parse addr.
fn parse_addr(value: &str, field: &'static str) -> Result<SocketAddr, String> {
    value
        .parse()
        .map_err(|error| format!("{field} must be a socket address: {error}"))
}

/// Escape a string for embedding in a TOML basic (double-quoted) string.
///
/// Handles the escapes that occur in the values this module writes — backslash,
/// double quote, and the control characters TOML requires be escaped. Basic
/// strings (unlike literal strings) can represent any value, including paths
/// containing apostrophes.
fn escape_toml_basic(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                write!(&mut out, "\\u{:04X}", c as u32)
                    .expect("writing to a String cannot fail");
            }
            c => out.push(c),
        }
    }
    out
}

/// Internal helper for home dir.
fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            "LOCALREF_CONFIG is not set and no home directory was found"
                .to_string()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_file_is_created_with_documented_defaults() {
        let temp = temp_config_dir("missing-config").join("config.toml");
        let config = LocalrefConfig::load_from_path(&temp).unwrap();

        assert_eq!(config.source_path(), temp.as_path());
        assert_eq!(config.workspace_name(), "Localref");
        let default_root =
            home_dir().unwrap().join(".localref").join("libroot");
        assert_eq!(config.library_root(), default_root);
        assert_eq!(
            config.plugins_dir(),
            default_root.join(".localref").join("plugins").as_path()
        );
        assert_eq!(config.rest_addr().to_string(), DEFAULT_REST_ADDR);
        assert_eq!(config.rest_endpoint(), "http://127.0.0.1:24817");
        assert_eq!(config.csc_addr().to_string(), DEFAULT_CSC_ADDR);
        assert!(config.desktop_start_hidden());
        assert!(config.desktop_quiet_start());
        assert_eq!(
            config.desktop_visible_columns(),
            ["Author", "Venue", "Year", "Type"]
        );
        assert_eq!(config.desktop_detail_width(), 420);
        assert_eq!(config.log_max_file_bytes(), DEFAULT_LOG_MAX_FILE_BYTES);
        assert_eq!(config.log_backup_count(), DEFAULT_LOG_BACKUP_COUNT);
        let written = std::fs::read_to_string(&temp).unwrap();
        assert!(written.contains("workspace_name = \"Localref\""));
        assert!(written.contains("library_root = "));
        assert!(written.contains("[rest]"));
        assert!(written.contains("[csc]"));
        assert!(written.contains("[desktop]"));
        assert!(written.contains("start_hidden = true"));
        assert!(written.contains("quiet_start = true"));
        assert!(written.contains("[logging]"));
        assert!(written.contains("max_file_bytes = 10485760"));
        assert!(written.contains("backup_count = 2"));

        std::fs::remove_dir_all(temp.parent().unwrap()).unwrap();
    }

    #[test]
    fn config_file_overrides_runtime_options() {
        let temp = tempfile_path("localref-config-overrides.toml");
        std::fs::write(
            &temp,
            r#"
library_root = "D:/LocalrefLibrary"
workspace_name = "Research Vault"

[rest]
addr = "127.0.0.1:3001"
endpoint = "http://localhost:3001"

[csc]
addr = "127.0.0.1:3002"

[desktop]
start_hidden = false
quiet_start = false
visible_columns = ["Author", "Categories", "Unknown"]
detail_width = 580

[logging]
max_file_bytes = 20971520
backup_count = 4
"#,
        )
        .unwrap();

        let config = LocalrefConfig::load_from_path(&temp).unwrap();

        assert_eq!(config.workspace_name(), "Research Vault");
        assert_eq!(config.library_root(), Path::new("D:/LocalrefLibrary"));
        assert_eq!(
            config.plugins_dir(),
            Path::new("D:/LocalrefLibrary/.localref/plugins")
        );
        assert_eq!(config.rest_addr().to_string(), "127.0.0.1:3001");
        assert_eq!(config.rest_endpoint(), "http://localhost:3001");
        assert_eq!(config.csc_addr().to_string(), "127.0.0.1:3002");
        assert!(!config.desktop_start_hidden());
        assert!(!config.desktop_quiet_start());
        assert_eq!(config.desktop_visible_columns(), ["Author", "Categories"]);
        assert_eq!(config.desktop_detail_width(), 580);
        assert_eq!(config.log_max_file_bytes(), 20 * 1024 * 1024);
        assert_eq!(config.log_backup_count(), 4);

        std::fs::remove_file(temp).unwrap();
    }

    #[test]
    fn invalid_socket_address_fails_loudly() {
        let temp = tempfile_path("localref-config-invalid.toml");
        std::fs::write(&temp, "[rest]\naddr = \"not an addr\"\n").unwrap();

        let error = LocalrefConfig::load_from_path(&temp).unwrap_err();

        assert!(error.contains("rest.addr must be a socket address"));

        std::fs::remove_file(temp).unwrap();
    }

    #[test]
    fn invalid_logging_limits_fail_loudly() {
        let temp = tempfile_path("localref-config-invalid-logging.toml");
        std::fs::write(
            &temp,
            "[logging]\nmax_file_bytes = 1024\nbackup_count = 11\n",
        )
        .unwrap();

        let error = LocalrefConfig::load_from_path(&temp).unwrap_err();

        assert!(
            error.contains("logging.max_file_bytes must be at least 65536")
        );
        std::fs::remove_file(temp).unwrap();
    }

    #[test]
    fn excessive_log_backup_count_fails_loudly() {
        let temp = tempfile_path("localref-config-invalid-log-backups.toml");
        std::fs::write(
            &temp,
            "[logging]\nmax_file_bytes = 10485760\nbackup_count = 11\n",
        )
        .unwrap();

        let error = LocalrefConfig::load_from_path(&temp).unwrap_err();

        assert!(
            error.contains("logging.backup_count must be between 0 and 10")
        );
        std::fs::remove_file(temp).unwrap();
    }

    #[test]
    fn save_round_trips_all_fields() {
        let temp = tempfile_path("localref-config-roundtrip.toml");
        std::fs::write(
            &temp,
            r#"
library_root = "D:/Vault"
workspace_name = "Round Trip"

[rest]
addr = "127.0.0.1:3001"
endpoint = "http://localhost:3001"

[csc]
addr = "127.0.0.1:3002"

[desktop]
start_hidden = false
quiet_start = false
"#,
        )
        .unwrap();
        let original = LocalrefConfig::load_from_path(&temp).unwrap();

        original.save().unwrap();
        let reloaded = LocalrefConfig::load_from_path(&temp).unwrap();

        assert_eq!(original, reloaded);

        std::fs::remove_file(temp).unwrap();
    }

    #[test]
    fn save_round_trips_library_root_with_apostrophe() {
        // A Windows username containing an apostrophe must survive save/reload.
        // A TOML literal string cannot hold `'`, so this caught the regression
        // where to_toml wrote an unparseable `'...O''Brien...'` value.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let mut config = LocalrefConfig::load_from_path(&path).unwrap();

        let root = Path::new(r"C:\Users\O'Brien\.localref\libroot");
        config.set_library_root(root);
        config.set_workspace_name("O'Brien's \"Vault\"").unwrap();
        config.save().unwrap();

        // The saved file must be valid TOML that reloads to the same value.
        let reloaded = LocalrefConfig::load_from_path(&path).unwrap();
        assert_eq!(reloaded.library_root(), root);
        assert_eq!(reloaded.workspace_name(), "O'Brien's \"Vault\"");
        assert_eq!(config, reloaded);
    }

    #[test]
    fn set_library_root_rederives_plugins_dir() {
        let temp = tempfile_path("localref-config-setroot.toml");
        let mut config = LocalrefConfig::load_from_path(&temp).unwrap();

        config.set_library_root(Path::new("E:/NewLib"));

        assert_eq!(config.library_root(), Path::new("E:/NewLib"));
        assert_eq!(
            config.plugins_dir(),
            Path::new("E:/NewLib/.localref/plugins")
        );

        std::fs::remove_file(temp).unwrap();
    }

    #[test]
    fn editable_settings_validate_and_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let mut config = LocalrefConfig::load_from_path(&path).unwrap();

        config.set_workspace_name("Research Library").unwrap();
        config.set_library_root(temp.path().join("library"));
        config.set_rest_addr("127.0.0.1:25000").unwrap();
        config.set_rest_endpoint("http://127.0.0.1:25000/").unwrap();
        config.set_csc_addr("127.0.0.1:24000").unwrap();
        config.set_desktop_start_hidden(false);
        config.set_desktop_quiet_start(false);
        config.set_desktop_visible_columns(vec![
            "Venue".to_string(),
            "Categories".to_string(),
        ]);
        config.set_desktop_detail_width(700);
        config.save().unwrap();

        let loaded = LocalrefConfig::load_from_path(&path).unwrap();
        assert_eq!(loaded.workspace_name(), "Research Library");
        assert_eq!(loaded.rest_addr().to_string(), "127.0.0.1:25000");
        assert_eq!(loaded.rest_endpoint(), "http://127.0.0.1:25000");
        assert_eq!(loaded.csc_addr().to_string(), "127.0.0.1:24000");
        assert!(!loaded.desktop_start_hidden());
        assert!(!loaded.desktop_quiet_start());
        assert_eq!(loaded.desktop_visible_columns(), ["Venue", "Categories"]);
        assert_eq!(loaded.desktop_detail_width(), 620);
    }

    #[test]
    fn legacy_repo_name_is_migrated_to_workspace_name_on_save() {
        let temp = tempfile_path("localref-config-legacy-name.toml");
        std::fs::write(&temp, "repo_name = \"Legacy Library\"\n").unwrap();

        let config = LocalrefConfig::load_from_path(&temp).unwrap();
        assert_eq!(config.workspace_name(), "Legacy Library");
        config.save().unwrap();

        let written = std::fs::read_to_string(&temp).unwrap();
        assert!(written.contains("workspace_name = \"Legacy Library\""));
        assert!(!written.contains("repo_name"));

        std::fs::remove_file(temp).unwrap();
    }

    fn tempfile_path(name: &str) -> PathBuf {
        let dir = temp_config_dir("files");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn temp_config_dir(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("localref-config-{}-{name}", std::process::id()))
    }
}
