//! Plugin configuration: which backend to sync to, its credentials, and this
//! device's client id.
//!
//! Stored at `<library>/.localref/s3sync/config.toml`. TOML is used so the
//! generated starter file can carry explanatory comments. Two backends are
//! supported, selected by `backend`:
//!
//! - `s3` — any S3-compatible store (AWS, Cloudflare R2, MinIO). Credentials come
//!   from the config (`access_key_id` / `secret_access_key` / optional
//!   `session_token`); the AWS environment chain is intentionally *not* consulted,
//!   so the config file is the single source of truth.
//! - `http` — a generic HTTP/WebDAV server (Nextcloud, ownCloud, Apache mod_dav),
//!   configured under `[http]` with a `url` and optional Basic-auth
//!   `username`/`password`.
//!
//! The `client_id` is generated once (into the starter template) and persisted.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which storage backend to sync to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// S3-compatible object storage (AWS, R2, MinIO).
    #[default]
    S3,
    /// Generic HTTP/WebDAV server.
    Http,
}

/// User-editable S3 sync configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3SyncConfig {
    /// Which backend to sync to. Defaults to `s3`.
    #[serde(default)]
    pub backend: Backend,
    /// Target bucket name (S3 backend).
    #[serde(default)]
    pub bucket: String,
    /// AWS region (e.g. `us-east-1`). Optional for S3-compatible endpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Custom endpoint URL for S3-compatible stores (MinIO, R2, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Allow plain HTTP (needed for local MinIO). Defaults to false.
    #[serde(default)]
    pub allow_http: bool,
    /// Key prefix under which all sync objects live in the bucket.
    #[serde(default)]
    pub prefix: String,
    /// Optional HTTP(S) proxy for reaching the S3 endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyConfig>,
    /// Access key id (S3 backend). Passed to the S3 client directly; the AWS
    /// environment chain is not consulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    /// Secret access key (S3 backend). Only used when `access_key_id` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    /// Optional session token, for temporary/STS credentials (S3 backend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    /// HTTP/WebDAV backend settings. Required when `backend = "http"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpConfig>,
    /// Stable per-device client id (generated on first run).
    #[serde(default)]
    pub client_id: String,
}

/// HTTP/WebDAV backend settings (`backend = "http"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    /// Base URL of the WebDAV collection, e.g. `https://dav.example.com/localref`.
    pub url: String,
    /// Optional Basic-auth username. When set, requests carry an
    /// `Authorization: Basic …` header built from `username:password`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Optional Basic-auth password (used only when `username` is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl HttpConfig {
    /// Build the `Authorization` header value for Basic auth, or `None` for
    /// anonymous access. Kept separate from the store builder so the encoding is
    /// unit-testable without a live server.
    pub fn auth_header(&self) -> Option<String> {
        use base64::Engine as _;
        let user = self.username.as_deref().filter(|u| !u.is_empty())?;
        let pass = self.password.as_deref().unwrap_or("");
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        Some(format!("Basic {encoded}"))
    }
}

/// Proxy for outbound requests. Assembled into a single proxy URL for
/// `object_store` (`scheme://host:port`). Proxy authentication is not supported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Proxy host or IP (e.g. `127.0.0.1`).
    pub host: String,
    /// Proxy port (e.g. `7890`).
    pub port: u16,
    /// URL scheme: `http`, `https`, or `socks5`. Defaults to `http`.
    #[serde(default = "default_proxy_scheme")]
    pub scheme: String,
}

/// Default proxy scheme when unspecified.
fn default_proxy_scheme() -> String {
    "http".to_owned()
}

impl ProxyConfig {
    /// Build the proxy URL for `object_store`. Returns an error if the assembled
    /// URL is invalid (bad scheme/host).
    pub fn to_url(&self) -> Result<String, String> {
        let url = url::Url::parse(&format!("{}://{}:{}", self.scheme, self.host, self.port))
            .map_err(|e| format!("invalid proxy host/port/scheme: {e}"))?;
        Ok(url.into())
    }
}

/// `<library>/.localref/s3sync/` — the plugin's private state directory.
pub fn plugin_dir(library_root: &Path) -> PathBuf {
    library_root.join(".localref").join("s3sync")
}

/// Path to the config file under the plugin state directory.
pub fn config_path(library_root: &Path) -> PathBuf {
    plugin_dir(library_root).join("config.toml")
}

impl S3SyncConfig {
    /// Load the config. If the file is missing, write a commented starter
    /// template for the user to fill in and return a descriptive error pointing
    /// at it, so the action surfaces setup instructions rather than failing
    /// opaquely.
    pub fn load(library_root: &Path) -> Result<Self, String> {
        let path = config_path(library_root);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => {
                // Only write a template when the file truly does not exist yet;
                // don't clobber an existing (but unreadable) file.
                if !path.exists() {
                    write_template(library_root)?;
                }
                return Err(format!(
                    "s3sync is not configured. A starter config was written to {}. \
                     Edit it (pick a `backend` and fill in its settings — `bucket` \
                     for s3, `[http].url` for http), then run the action again.",
                    path.display()
                ));
            }
        };
        let mut config: Self =
            toml::from_str(&text).map_err(|e| format!("invalid s3sync config: {e}"))?;
        match config.backend {
            Backend::S3 => {
                if config.bucket.trim().is_empty() {
                    return Err(format!(
                        "s3sync config at {} uses backend `s3` but has an empty `bucket`; \
                         fill it in before syncing.",
                        path.display()
                    ));
                }
            }
            Backend::Http => {
                let url_ok =
                    config.http.as_ref().is_some_and(|h| !h.url.trim().is_empty());
                if !url_ok {
                    return Err(format!(
                        "s3sync config at {} uses backend `http` but `[http].url` is \
                         missing or empty; set it before syncing.",
                        path.display()
                    ));
                }
            }
        }
        // Backfill a client id on first load and persist it.
        if config.client_id.trim().is_empty() {
            config.client_id = generate_client_id();
            config.save(library_root)?;
        }
        Ok(config)
    }

    /// Persist the config back to disk as TOML.
    pub fn save(&self, library_root: &Path) -> Result<(), String> {
        let dir = plugin_dir(library_root);
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(config_path(library_root), text).map_err(|e| e.to_string())
    }
}

/// Commented starter config written when none exists, so the user has an
/// annotated file to fill in rather than a blank one.
const CONFIG_TEMPLATE: &str = "\
# s3sync configuration.

# Which backend to sync to: \"s3\" (S3-compatible) or \"http\" (HTTP/WebDAV).
backend = \"s3\"

# ── S3 backend (backend = \"s3\") ─────────────────────────────────────────────

# Target bucket name (required for s3). For local testing you may use a
# \"file:///abs/path\" pseudo-bucket instead of a real S3 bucket.
bucket = \"\"

# AWS region, e.g. \"us-east-1\". Optional for S3-compatible endpoints.
# For Cloudflare R2 use \"auto\".
# region = \"us-east-1\"

# Custom endpoint URL for S3-compatible stores (MinIO, Cloudflare R2, …).
# For R2: \"https://<accountid>.r2.cloudflarestorage.com\"
# endpoint = \"http://127.0.0.1:9000\"

# Credentials for the s3 backend. These are the only source of credentials —
# the AWS environment chain is not consulted.
# access_key_id = \"\"
# secret_access_key = \"\"
# session_token = \"\"   # only for temporary/STS credentials

# ── HTTP/WebDAV backend (backend = \"http\") ──────────────────────────────────

# Uncomment and set when backend = \"http\". `url` is the WebDAV collection base;
# username/password enable Basic auth (leave blank for anonymous access).
# [http]
# url = \"https://dav.example.com/localref\"
# username = \"\"
# password = \"\"

# ── Shared ───────────────────────────────────────────────────────────────────

# Allow plain HTTP (needed for local MinIO or an http:// WebDAV server).
allow_http = false

# Key prefix under which all sync objects live.
prefix = \"\"

# Optional proxy for reaching the backend. Remove this section to connect
# directly. `scheme` defaults to \"http\". Proxy authentication is not supported.
# [proxy]
# host = \"127.0.0.1\"
# port = 7890
# scheme = \"http\"

# Stable per-device client id. Leave empty — it is generated on first use.
client_id = \"\"
";

/// Write the commented starter template to the config path, creating the
/// plugin state directory if needed.
fn write_template(library_root: &Path) -> Result<(), String> {
    let dir = plugin_dir(library_root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    std::fs::write(config_path(library_root), CONFIG_TEMPLATE).map_err(|e| e.to_string())
}

/// Generate a stable-ish client id from the hostname plus a time salt. Uniqueness
/// only needs to hold across devices sharing one bucket, so hostname + a
/// coarse timestamp is sufficient; it is persisted immediately after.
fn generate_client_id() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "device".to_owned());
    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let sanitized: String = host.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("{sanitized}-{salt:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(scheme: &str) -> ProxyConfig {
        ProxyConfig { host: "127.0.0.1".to_owned(), port: 7890, scheme: scheme.to_owned() }
    }

    fn http(user: Option<&str>, pass: Option<&str>) -> HttpConfig {
        HttpConfig {
            url: "https://dav.example.com/localref".to_owned(),
            username: user.map(str::to_owned),
            password: pass.map(str::to_owned),
        }
    }

    #[test]
    fn to_url_has_no_userinfo() {
        // Proxy auth was dropped; the URL is a plain scheme://host:port.
        assert_eq!(proxy("http").to_url().unwrap(), "http://127.0.0.1:7890/");
        // Non-special schemes (socks5) get no trailing-slash normalization.
        assert_eq!(proxy("socks5").to_url().unwrap(), "socks5://127.0.0.1:7890");
    }

    #[test]
    fn auth_header_encodes_basic_credentials() {
        // base64("alice:s3cr3t") — Basic auth is what WebDAV servers expect.
        assert_eq!(http(Some("alice"), Some("s3cr3t")).auth_header().unwrap(), "Basic YWxpY2U6czNjcjN0");
    }

    #[test]
    fn auth_header_absent_without_username() {
        // No username means anonymous access — no Authorization header at all.
        assert!(http(None, None).auth_header().is_none());
        assert!(http(Some(""), Some("x")).auth_header().is_none());
    }

    #[test]
    fn backend_defaults_to_s3_when_omitted() {
        // An older config without a `backend` key must keep working as s3.
        let cfg: S3SyncConfig = toml::from_str("bucket = \"b\"").unwrap();
        assert_eq!(cfg.backend, Backend::S3);
    }

    #[test]
    fn backend_parses_http_lowercase() {
        let cfg: S3SyncConfig =
            toml::from_str("backend = \"http\"\n[http]\nurl = \"https://x\"").unwrap();
        assert_eq!(cfg.backend, Backend::Http);
        assert_eq!(cfg.http.unwrap().url, "https://x");
    }
}
