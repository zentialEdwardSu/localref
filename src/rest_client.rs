//! Blocking REST client for Localref REST entry points.
//!
//! The client intentionally speaks only to the Localref REST API. It does not
//! read or write the library filesystem, preserving the process boundary.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use localref_core::config::LocalrefConfig;
use localref_core::model::{
    Event, ItemDocument, ItemFilesDocument, Metadata, MetadataDocument,
    SearchHit,
};
pub use localref_core::storage::CategorySummary;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Snapshot rendered by the simple desktop dashboard.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct DashboardSnapshot {
    /// Indexed item count.
    pub item_count: usize,
    /// Category count.
    pub category_count: usize,
    /// Pending import count.
    pub pending_count: usize,
    /// Recent event count.
    pub event_count: usize,
}

/// Pending import summary returned by the REST API.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PendingImportSummary {
    /// Pending import id.
    pub id: u64,
    /// Item title.
    pub title: String,
    /// Imported item type, when known.
    pub item_type: Option<String>,
    /// Source URI, when known.
    pub uri: Option<String>,
    /// Rule-suggested categories.
    pub suggested_categories: Vec<String>,
}

/// Request body used to patch one metadata document.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
struct MetadataPatchRequest {
    /// Revision hash observed by the UI before editing.
    expected_revision: String,
    /// Complete metadata replacement.
    metadata: Metadata,
}

/// Request body used by category add operations.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct CategoryRequest {
    /// Category path relative to `Cat/`.
    category: String,
}

/// Request body used by file import and file-open operations.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct FilePathRequest {
    /// Path accepted by the target endpoint.
    path: String,
}

/// Daemon queue status returned by `/api/daemon/status`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct DaemonStatus {
    /// Whether a daemon task is currently running.
    pub running: bool,
    /// Number of queued daemon tasks.
    pub queued_tasks: usize,
    /// Recent task records as API JSON.
    #[serde(default)]
    pub recent_tasks: Vec<serde_json::Value>,
    /// Active pause modes.
    #[serde(default)]
    pub paused_modes: Vec<String>,
}

/// Request body used by daemon pause and resume endpoints.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct PauseRequest {
    /// Pause mode to add or remove.
    mode: String,
}

/// Small blocking REST client for localhost desktop entry points.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestClient {
    endpoint: String,
}

impl RestClient {
    /// Create a REST client from a base endpoint.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self { endpoint: endpoint.into().trim_end_matches('/').to_string() }
    }

    /// Create a client from an already loaded Localref configuration.
    pub fn from_config(config: &LocalrefConfig) -> Self {
        Self::new(config.rest_endpoint())
    }

    /// Create a client by loading the Localref configuration file.
    pub fn from_config_file() -> Result<Self, String> {
        LocalrefConfig::load().map(|config| Self::from_config(&config))
    }

    /// Return the configured endpoint.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Fetch a dashboard snapshot from REST.
    pub fn dashboard_snapshot(&self) -> Result<DashboardSnapshot, String> {
        Ok(DashboardSnapshot {
            item_count: self.list_items()?.len(),
            category_count: self.list_categories()?.len(),
            pending_count: self.list_pending_imports()?.len(),
            event_count: self.list_events()?.len(),
        })
    }

    /// Return all indexed item documents.
    pub fn list_items(&self) -> Result<Vec<ItemDocument>, String> {
        self.get_json("/api/items")
    }

    /// Return the full metadata document for one item.
    pub fn get_metadata(
        &self,
        item_id: &str,
    ) -> Result<MetadataDocument, String> {
        self.get_json(&format!("/api/items/{}/metadata", encode_path(item_id)))
    }

    /// Return files currently present in one item directory.
    pub fn list_item_files(
        &self,
        item_id: &str,
    ) -> Result<ItemFilesDocument, String> {
        self.get_json(&format!("/api/items/{}/files", encode_path(item_id)))
    }

    /// Open an item-relative file path with the system viewer.
    pub fn open_item_file(
        &self,
        item_id: &str,
        path: impl Into<String>,
    ) -> Result<Value, String> {
        self.post_json(
            &format!("/api/items/{}/files/open", encode_path(item_id)),
            &FilePathRequest { path: path.into() },
        )
    }

    /// Copy one file into an existing item directory.
    pub fn add_item_file(
        &self,
        item_id: &str,
        path: impl Into<String>,
    ) -> Result<ItemDocument, String> {
        self.post_json(
            &format!("/api/items/{}/files", encode_path(item_id)),
            &FilePathRequest { path: path.into() },
        )
    }

    /// Open one item directory with the system file manager.
    pub fn open_item_folder(&self, item_id: &str) -> Result<Value, String> {
        self.post_json(
            &format!("/api/items/{}/folder/open", encode_path(item_id)),
            &Value::Null,
        )
    }

    /// Patch a metadata document through the daemon queue.
    pub fn patch_metadata(
        &self,
        item_id: &str,
        expected_revision: impl Into<String>,
        metadata: Metadata,
    ) -> Result<ItemDocument, String> {
        let request = MetadataPatchRequest {
            expected_revision: expected_revision.into(),
            metadata,
        };
        self.patch_json(
            &format!("/api/items/{}/metadata", encode_path(item_id)),
            &request,
        )
    }

    /// Return category paths derived from `Cat/`.
    pub fn list_categories(&self) -> Result<Vec<CategorySummary>, String> {
        self.get_json("/api/categories/tree")
    }

    /// Create one empty category.
    pub fn create_category(
        &self,
        category: impl Into<String>,
    ) -> Result<CategorySummary, String> {
        self.post_json(
            "/api/categories",
            &CategoryRequest { category: category.into() },
        )
    }

    /// Add one item to one category.
    pub fn add_item_category(
        &self,
        item_id: &str,
        category: impl Into<String>,
    ) -> Result<CategorySummary, String> {
        self.post_json(
            &format!("/api/items/{}/categories", encode_path(item_id)),
            &CategoryRequest { category: category.into() },
        )
    }

    /// Remove one item from one category.
    pub fn remove_item_category(
        &self,
        item_id: &str,
        category: &str,
    ) -> Result<CategorySummary, String> {
        self.delete_json(&format!(
            "/api/items/{}/categories/{}",
            encode_path(item_id),
            encode_path_preserving_slash(category)
        ))
    }

    /// Return pending connector imports waiting for user confirmation.
    pub fn list_pending_imports(
        &self,
    ) -> Result<Vec<PendingImportSummary>, String> {
        self.get_json("/api/import/pending")
    }

    /// Return recent daemon events.
    pub fn list_events(&self) -> Result<Vec<Event>, String> {
        self.get_json("/api/events")
    }

    /// Search indexed items.
    pub fn search(&self, term: &str) -> Result<Vec<SearchHit>, String> {
        self.get_json(&format!("/api/search?q={}", encode_query(term)))
    }

    /// Request a daemon scan.
    pub fn scan(&self) -> Result<Value, String> {
        self.post_json("/api/daemon/scan", &Value::Null)
    }

    /// Return daemon queue and pause status.
    pub fn daemon_status(&self) -> Result<DaemonStatus, String> {
        self.get_json("/api/daemon/status")
    }

    /// Pause one daemon mode.
    pub fn pause(
        &self,
        mode: impl Into<String>,
    ) -> Result<DaemonStatus, String> {
        self.post_json(
            "/api/daemon/pause",
            &PauseRequest { mode: mode.into() },
        )
    }

    /// Resume one daemon mode.
    pub fn resume(
        &self,
        mode: impl Into<String>,
    ) -> Result<DaemonStatus, String> {
        self.post_json(
            "/api/daemon/resume",
            &PauseRequest { mode: mode.into() },
        )
    }

    /// Fetch JSON from one API path.
    pub fn get_json<T>(&self, path: &str) -> Result<T, String>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.request_json("GET", path, None)
    }

    /// Post JSON to one API path and decode the JSON response.
    pub fn post_json<T, B>(&self, path: &str, body: &B) -> Result<T, String>
    where
        T: for<'de> Deserialize<'de>,
        B: serde::Serialize,
    {
        let body =
            serde_json::to_string(body).map_err(|error| error.to_string())?;
        self.request_json("POST", path, Some(&body))
    }

    /// Patch JSON to one API path and decode the JSON response.
    pub fn patch_json<T, B>(&self, path: &str, body: &B) -> Result<T, String>
    where
        T: for<'de> Deserialize<'de>,
        B: serde::Serialize,
    {
        let body =
            serde_json::to_string(body).map_err(|error| error.to_string())?;
        self.request_json("PATCH", path, Some(&body))
    }

    /// Send a JSON DELETE request and decode the JSON response.
    pub fn delete_json<T>(&self, path: &str) -> Result<T, String>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.request_json("DELETE", path, None)
    }

    /// Send one HTTP request and decode a JSON response.
    fn request_json<T>(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<T, String>
    where
        T: for<'de> Deserialize<'de>,
    {
        let response = self.request(method, path, body)?;
        serde_json::from_str(&response.body).map_err(|error| error.to_string())
    }

    /// Send one raw HTTP request to the configured REST endpoint.
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<HttpResponse, String> {
        let target = HttpTarget::parse(&self.endpoint, path)?;
        let mut stream =
            TcpStream::connect((target.host.as_str(), target.port))
                .map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|error| error.to_string())?;
        let body = body.unwrap_or("");
        let request = format!(
            "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            target.path,
            target.host,
            body.len(),
            body
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|error| error.to_string())?;
        let mut raw = String::new();
        stream.read_to_string(&mut raw).map_err(|error| error.to_string())?;
        parse_http_response(&raw)
    }
}

#[derive(Debug, Eq, PartialEq)]
struct HttpTarget {
    host: String,
    port: u16,
    path: String,
}

#[derive(Debug, Eq, PartialEq)]
struct HttpResponse {
    status: u16,
    body: String,
}

impl HttpTarget {
    /// Parse a base endpoint plus API path into connection coordinates.
    fn parse(endpoint: &str, path: &str) -> Result<Self, String> {
        let endpoint = endpoint.strip_prefix("http://").ok_or_else(|| {
            "only http:// endpoints are supported".to_string()
        })?;
        let (host_port, base_path) =
            endpoint.split_once('/').unwrap_or((endpoint, ""));
        let (host, port) = host_port
            .split_once(':')
            .map(|(host, port)| {
                port.parse::<u16>()
                    .map(|port| (host.to_string(), port))
                    .map_err(|error| error.to_string())
            })
            .unwrap_or_else(|| Ok((host_port.to_string(), 80)))?;
        let path = format!(
            "/{}{}",
            base_path.trim_matches('/'),
            path_with_leading_slash(path)
        )
        .replace("//", "/");
        Ok(Self { host, port, path })
    }
}

/// Ensure an API path starts with `/`.
fn path_with_leading_slash(path: &str) -> String {
    if path.starts_with('/') { path.to_string() } else { format!("/{path}") }
}

/// Parse a complete HTTP response and return its body.
fn parse_http_response(raw: &str) -> Result<HttpResponse, String> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "invalid HTTP response".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| "missing HTTP status".to_string())?
        .parse::<u16>()
        .map_err(|error| error.to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}: {body}"));
    }
    Ok(HttpResponse { status, body: body.to_string() })
}

/// Percent-encode one path component.
fn encode_path(value: &str) -> String {
    percent_encode(value, false)
}

/// Percent-encode a path while preserving hierarchy separators.
fn encode_path_preserving_slash(value: &str) -> String {
    percent_encode(value, true)
}

/// Percent-encode one query parameter value.
fn encode_query(value: &str) -> String {
    percent_encode(value, false)
}

/// Percent-encode bytes not allowed in Localref desktop request paths.
fn percent_encode(value: &str, preserve_slash: bool) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':')
            || (preserve_slash && byte == b'/');
        if keep {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_endpoint_targets() {
        assert_eq!(
            HttpTarget::parse("http://127.0.0.1:24817", "/api/items").unwrap(),
            HttpTarget {
                host: "127.0.0.1".to_string(),
                port: 24817,
                path: "/api/items".to_string(),
            }
        );
    }

    #[test]
    fn parses_http_response_body() {
        let response = parse_http_response(
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
        )
        .unwrap();
        assert_eq!(response.body, "{}");
    }

    #[test]
    fn encodes_path_and_query_values() {
        assert_eq!(encode_path("lr:test:one two"), "lr:test:one%20two");
        assert_eq!(encode_query("near field"), "near%20field");
        assert_eq!(
            encode_path_preserving_slash("Wireless/RIS 2026"),
            "Wireless/RIS%202026"
        );
    }
}
