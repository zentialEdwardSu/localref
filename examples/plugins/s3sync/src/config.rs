//! Plugin configuration: where the S3 bucket is and this device's client id.
//!
//! Stored at `<library>/.localref/s3sync/config.toml`. TOML is used so the
//! generated starter file can carry explanatory comments. Credentials may be
//! set here (`access_key_id` / `secret_access_key` / optional `session_token`)
//! to make configuring an S3-compatible store like Cloudflare R2 a single-file
//! edit; when left blank they fall back to the standard AWS environment chain
//! (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN`). The
//! `client_id` is generated once (into the starter template) and persisted.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// User-editable S3 sync configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3SyncConfig {
    /// Target bucket name.
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
    /// Access key id. When set, it is passed to the S3 client directly;
    /// otherwise credentials come from the AWS environment chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    /// Secret access key. Only used when `access_key_id` is also set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    /// Optional session token, for temporary/STS credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    /// Stable per-device client id (generated on first run).
    #[serde(default)]
    pub client_id: String,
}

/// Proxy for outbound S3 requests. Assembled into a single proxy URL for
/// `object_store` (`scheme://[user:pass@]host:port`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Proxy host or IP (e.g. `127.0.0.1`).
    pub host: String,
    /// Proxy port (e.g. `7890`).
    pub port: u16,
    /// URL scheme: `http`, `https`, or `socks5`. Defaults to `http`.
    #[serde(default = "default_proxy_scheme")]
    pub scheme: String,
    /// Optional proxy username for authenticating proxies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Optional proxy password (used only when `username` is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// Default proxy scheme when unspecified.
fn default_proxy_scheme() -> String {
    "http".to_owned()
}

impl ProxyConfig {
    /// Build the proxy URL for `object_store`, percent-encoding any credentials.
    /// Returns an error if the assembled URL is invalid (bad scheme/host).
    pub fn to_url(&self) -> Result<String, String> {
        let mut url = url::Url::parse(&format!("{}://{}:{}", self.scheme, self.host, self.port))
            .map_err(|e| format!("invalid proxy host/port/scheme: {e}"))?;
        if let Some(user) = self.username.as_deref().filter(|u| !u.is_empty()) {
            url.set_username(user)
                .map_err(|()| "cannot set proxy username on this URL".to_owned())?;
            // A password is only meaningful alongside a username.
            url.set_password(self.password.as_deref())
                .map_err(|()| "cannot set proxy password on this URL".to_owned())?;
        }
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
                     Edit it (set `bucket` at minimum), set AWS credentials via \
                     environment variables, then run the action again.",
                    path.display()
                ));
            }
        };
        let mut config: Self =
            toml::from_str(&text).map_err(|e| format!("invalid s3sync config: {e}"))?;
        if config.bucket.trim().is_empty() {
            return Err(format!(
                "s3sync config at {} has an empty `bucket`; fill it in before syncing.",
                path.display()
            ));
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

# Target bucket name (required). For local testing you may use a
# \"file:///abs/path\" pseudo-bucket instead of a real S3 bucket.
bucket = \"\"

# AWS region, e.g. \"us-east-1\". Optional for S3-compatible endpoints.
# For Cloudflare R2 use \"auto\".
# region = \"us-east-1\"

# Custom endpoint URL for S3-compatible stores (MinIO, Cloudflare R2, …).
# For R2: \"https://<accountid>.r2.cloudflarestorage.com\"
# endpoint = \"http://127.0.0.1:9000\"

# Allow plain HTTP (needed for local MinIO). Defaults to false.
allow_http = false

# Key prefix under which all sync objects live in the bucket.
prefix = \"\"

# Credentials. Set these to configure an S3-compatible store (e.g. Cloudflare
# R2) in one place. Leave them blank/commented to use the AWS environment chain
# instead (AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN).
# access_key_id = \"\"
# secret_access_key = \"\"
# session_token = \"\"   # only for temporary/STS credentials

# Optional HTTP(S) proxy for reaching the S3 endpoint. Remove this section to
# connect directly. `scheme` defaults to \"http\"; username/password are only
# needed for authenticating proxies.
# [proxy]
# host = \"127.0.0.1\"
# port = 7890
# scheme = \"http\"
# username = \"\"
# password = \"\"

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

    fn proxy(scheme: &str, user: Option<&str>, pass: Option<&str>) -> ProxyConfig {
        ProxyConfig {
            host: "127.0.0.1".to_owned(),
            port: 7890,
            scheme: scheme.to_owned(),
            username: user.map(str::to_owned),
            password: pass.map(str::to_owned),
        }
    }

    #[test]
    fn to_url_without_auth_omits_userinfo() {
        // A bare proxy must not grow an empty `@` that reqwest would reject.
        assert_eq!(proxy("http", None, None).to_url().unwrap(), "http://127.0.0.1:7890/");
    }

    #[test]
    fn to_url_embeds_credentials() {
        let url = proxy("http", Some("alice"), Some("s3cr3t")).to_url().unwrap();
        assert_eq!(url, "http://alice:s3cr3t@127.0.0.1:7890/");
    }

    #[test]
    fn to_url_percent_encodes_special_characters() {
        // Credentials with `@`/`:` must be encoded so the host is still parsed
        // as 127.0.0.1, not smuggled in via the password.
        let url = proxy("http", Some("user@corp"), Some("p:@ss")).to_url().unwrap();
        assert_eq!(url, "http://user%40corp:p%3A%40ss@127.0.0.1:7890/");
    }
}
