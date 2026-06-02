//! Configuration loading for Localref entry points.
//!
//! This crate is the single place that reads Localref process configuration.
//! `LOCALREF_CONFIG` selects the TOML file. When it is not set, Localref reads
//! `~/.localref/config.toml`. Missing files are created with documented
//! defaults; malformed files or invalid values fail loudly.

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

/// Runtime configuration shared by Localref binaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalrefConfig {
    source_path: PathBuf,
    repo_name: String,
    library_root: PathBuf,
    rest_addr: SocketAddr,
    rest_endpoint: String,
    csc_addr: SocketAddr,
    desktop_start_hidden: bool,
    desktop_quiet_start: bool,
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    repo_name: Option<String>,
    library_root: Option<PathBuf>,
    rest: Option<RestConfigFile>,
    csc: Option<CscConfigFile>,
    desktop: Option<DesktopConfigFile>,
}

#[derive(Debug, Default, Deserialize)]
struct RestConfigFile {
    addr: Option<String>,
    endpoint: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CscConfigFile {
    addr: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DesktopConfigFile {
    start_hidden: Option<bool>,
    quiet_start: Option<bool>,
}

impl LocalrefConfig {
    /// Load configuration from `LOCALREF_CONFIG` or the default path.
    pub fn load() -> Result<Self, String> {
        let path = config_path()?;
        Self::load_from_path(path)
    }

    /// Load configuration from one explicit TOML file path.
    pub fn load_from_path(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        if !path.exists() {
            let file = ConfigFile::default();
            let config = Self::from_config_file(path, file)?;
            write_default_config(&config)?;
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

    /// Return the file that supplied this configuration.
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// Return the configured Localref library root.
    pub fn library_root(&self) -> &Path {
        &self.library_root
    }

    /// Return the configured repository display name.
    pub fn repo_name(&self) -> &str {
        &self.repo_name
    }

    /// Return the REST API bind address for server binaries.
    pub fn rest_addr(&self) -> SocketAddr {
        self.rest_addr
    }

    /// Return the REST API endpoint for desktop clients.
    pub fn rest_endpoint(&self) -> &str {
        &self.rest_endpoint
    }

    /// Return the Zotero Connector-compatible bind address.
    pub fn csc_addr(&self) -> SocketAddr {
        self.csc_addr
    }

    /// Return whether tray-hosted startup should skip the initial window.
    pub fn desktop_start_hidden(&self) -> bool {
        self.desktop_start_hidden
    }

    /// Return whether tray-hosted startup should avoid console chatter.
    pub fn desktop_quiet_start(&self) -> bool {
        self.desktop_quiet_start
    }

    fn from_config_file(
        source_path: PathBuf,
        file: ConfigFile,
    ) -> Result<Self, String> {
        let library_root =
            file.library_root.unwrap_or(default_library_root()?);
        let repo_name = file
            .repo_name
            .and_then(|value| optional_text(&value))
            .unwrap_or_else(|| "Localref".to_string());
        let rest = file.rest.unwrap_or_default();
        let csc = file.csc.unwrap_or_default();
        let desktop = file.desktop.unwrap_or_default();
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
        Ok(Self {
            source_path,
            repo_name,
            library_root,
            rest_addr,
            rest_endpoint,
            csc_addr,
            desktop_start_hidden: desktop.start_hidden.unwrap_or(true),
            desktop_quiet_start: desktop.quiet_start.unwrap_or(true),
        })
    }
}

/// Return the configured TOML path from `LOCALREF_CONFIG` or `~/.localref`.
pub fn config_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(LOCALREF_CONFIG_ENV) {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".localref").join("config.toml"))
}

fn parse_addr(value: &str, field: &'static str) -> Result<SocketAddr, String> {
    value
        .parse()
        .map_err(|error| format!("{field} must be a socket address: {error}"))
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            "LOCALREF_CONFIG is not set and no home directory was found"
                .to_string()
        })
}

fn default_library_root() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".localref").join("libroot"))
}

fn write_default_config(config: &LocalrefConfig) -> Result<(), String> {
    if let Some(parent) = config.source_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!("failed to create {}: {error}", parent.display())
        })?;
    }
    let text = format!(
        "repo_name = \"{}\"\nlibrary_root = '{}'\n\n[rest]\naddr = \"{}\"\nendpoint = \"{}\"\n\n[csc]\naddr = \"{}\"\n\n[desktop]\nstart_hidden = {}\nquiet_start = {}\n",
        toml_basic_string(&config.repo_name),
        toml_literal_path(&config.library_root),
        config.rest_addr,
        config.rest_endpoint,
        config.csc_addr,
        config.desktop_start_hidden,
        config.desktop_quiet_start
    );
    std::fs::write(&config.source_path, text).map_err(|error| {
        format!("failed to write {}: {error}", config.source_path.display())
    })
}

fn toml_literal_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn toml_basic_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() { None } else { Some(value.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_file_is_created_with_documented_defaults() {
        let temp = temp_config_dir("missing-config").join("config.toml");
        let config = LocalrefConfig::load_from_path(&temp).unwrap();

        assert_eq!(config.source_path(), temp.as_path());
        assert_eq!(config.repo_name(), "Localref");
        assert_eq!(config.library_root(), default_library_root().unwrap());
        assert_eq!(config.rest_addr().to_string(), DEFAULT_REST_ADDR);
        assert_eq!(config.rest_endpoint(), "http://127.0.0.1:24817");
        assert_eq!(config.csc_addr().to_string(), DEFAULT_CSC_ADDR);
        assert!(config.desktop_start_hidden());
        assert!(config.desktop_quiet_start());
        let written = std::fs::read_to_string(&temp).unwrap();
        assert!(written.contains("repo_name = \"Localref\""));
        assert!(written.contains("library_root = "));
        assert!(written.contains("[rest]"));
        assert!(written.contains("[csc]"));
        assert!(written.contains("[desktop]"));
        assert!(written.contains("start_hidden = true"));
        assert!(written.contains("quiet_start = true"));

        std::fs::remove_dir_all(temp.parent().unwrap()).unwrap();
    }

    #[test]
    fn config_file_overrides_runtime_options() {
        let temp = tempfile_path("localref-config-overrides.toml");
        std::fs::write(
            &temp,
            r#"
library_root = "D:/LocalrefLibrary"
repo_name = "Research Vault"

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

        let config = LocalrefConfig::load_from_path(&temp).unwrap();

        assert_eq!(config.repo_name(), "Research Vault");
        assert_eq!(config.library_root(), Path::new("D:/LocalrefLibrary"));
        assert_eq!(config.rest_addr().to_string(), "127.0.0.1:3001");
        assert_eq!(config.rest_endpoint(), "http://localhost:3001");
        assert_eq!(config.csc_addr().to_string(), "127.0.0.1:3002");
        assert!(!config.desktop_start_hidden());
        assert!(!config.desktop_quiet_start());

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
