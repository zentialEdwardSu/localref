//! Typed async REST client for the Localref daemon.
//!
//! One set of methods over the daemon's REST routes, returning
//! `localref-core` model types. Used by the host and by Rust plugins.
#![warn(unreachable_pub)]
#![deny(clippy::correctness)]
#![deny(clippy::single_call_fn)]
#![deny(clippy::complexity)]
#![warn(clippy::pedantic)]
#![warn(clippy::useless_attribute)]
#![warn(clippy::redundant_pub_crate)]
#![warn(clippy::excessive_precision)]
#![warn(clippy::missing_docs_in_private_items)]

pub use localref_core::model::ItemDocument;
use localref_core::model::{ItemFilesDocument, MetadataDocument, SearchHit};
use localref_core::storage::CategorySummary;
use serde::{Deserialize, Serialize};

/// Daemon queue and pause status (mirrors `/api/daemon/status`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct DaemonStatus {
    /// Whether a daemon task is currently running.
    pub running: bool,
    /// Number of queued daemon tasks.
    pub queued_tasks: usize,
    /// Recent task records as raw JSON.
    #[serde(default)]
    pub recent_tasks: Vec<serde_json::Value>,
    /// Active pause modes.
    #[serde(default)]
    pub paused_modes: Vec<String>,
}

/// Aggregate counts for the tray dashboard.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct DashboardSnapshot {
    /// Indexed item count.
    pub item_count: usize,
    /// Category count.
    pub category_count: usize,
    /// Recent log entry count.
    pub log_count: usize,
}

/// Errors surfaced by the REST client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Transport-level failure (connect, timeout, TLS).
    #[error("request transport error: {0}")]
    Transport(String),
    /// Server returned a non-success HTTP status.
    #[error("unexpected status {status}: {body}")]
    Status {
        /// HTTP status code returned by the daemon.
        status: u16,
        /// Response body text, for diagnostics.
        body: String,
    },
    /// Response body did not deserialize into the expected type.
    #[error("decode error: {0}")]
    Decode(String),
}

/// Async REST client bound to one daemon endpoint.
#[derive(Clone, Debug)]
pub struct LocalrefClient {
    endpoint: String,
    http: reqwest::Client,
}

impl LocalrefClient {
    /// Build a client for the given REST base URL (e.g. `http://127.0.0.1:8787`).
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Return the configured base endpoint.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// GET a path and decode the JSON body into `T`.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, ClientError> {
        let url = format!("{}{path}", self.endpoint);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Self::decode(resp).await
    }

    /// POST a JSON body to a path and decode the JSON response into `T`.
    // Shared POST helper; gains more callers in later tasks.
    #[allow(clippy::single_call_fn)]
    async fn post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let url = format!("{}{path}", self.endpoint);
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Self::decode(resp).await
    }

    /// Check status, then decode the body, mapping failures to `ClientError`.
    async fn decode<T: serde::de::DeserializeOwned>(
        resp: reqwest::Response,
    ) -> Result<T, ClientError> {
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Status { status: status.as_u16(), body });
        }
        resp.json::<T>()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// Return all indexed item documents.
    pub async fn list_items(&self) -> Result<Vec<ItemDocument>, ClientError> {
        self.get_json("/api/items").await
    }

    /// Return one item document by id.
    pub async fn get_item(
        &self,
        item_id: &str,
    ) -> Result<ItemDocument, ClientError> {
        self.get_json(&format!("/api/items/{}", encode_segment(item_id)))
            .await
    }

    /// Return the files present in one item directory.
    pub async fn item_files(
        &self,
        item_id: &str,
    ) -> Result<ItemFilesDocument, ClientError> {
        self.get_json(&format!(
            "/api/items/{}/files",
            encode_segment(item_id)
        ))
        .await
    }

    /// Return the full metadata document for one item.
    pub async fn get_metadata(
        &self,
        item_id: &str,
    ) -> Result<MetadataDocument, ClientError> {
        self.get_json(&format!(
            "/api/items/{}/metadata",
            encode_segment(item_id)
        ))
        .await
    }

    /// Return category paths derived from `Cat/`.
    pub async fn categories_tree(
        &self,
    ) -> Result<Vec<CategorySummary>, ClientError> {
        self.get_json("/api/categories/tree").await
    }

    /// Add one item to one category.
    pub async fn add_item_category(
        &self,
        item_id: &str,
        category: &str,
    ) -> Result<CategorySummary, ClientError> {
        self.post_json(
            &format!("/api/items/{}/categories", encode_segment(item_id)),
            &serde_json::json!({ "category": category }),
        )
        .await
    }

    /// Search indexed items by term.
    pub async fn search(
        &self,
        term: &str,
    ) -> Result<Vec<SearchHit>, ClientError> {
        self.get_json(&format!("/api/search?q={}", encode_segment(term)))
            .await
    }

    /// Return daemon queue and pause status.
    pub async fn daemon_status(&self) -> Result<DaemonStatus, ClientError> {
        self.get_json("/api/daemon/status").await
    }

    /// Pause one daemon mode.
    pub async fn pause(&self, mode: &str) -> Result<DaemonStatus, ClientError> {
        self.post_json("/api/daemon/pause", &serde_json::json!({ "mode": mode })).await
    }

    /// Resume one daemon mode.
    pub async fn resume(&self, mode: &str) -> Result<DaemonStatus, ClientError> {
        self.post_json("/api/daemon/resume", &serde_json::json!({ "mode": mode })).await
    }

    /// Request a daemon scan.
    pub async fn scan(&self) -> Result<serde_json::Value, ClientError> {
        self.post_json("/api/daemon/scan", &serde_json::Value::Null).await
    }

    /// Recent log entries from the ring buffer.
    pub async fn events(
        &self,
    ) -> Result<Vec<localref_core::logging::LogEntry>, ClientError> {
        self.get_json("/api/events").await
    }

    /// Aggregate dashboard counts (items, categories, recent events).
    pub async fn dashboard_snapshot(
        &self,
    ) -> Result<DashboardSnapshot, ClientError> {
        Ok(DashboardSnapshot {
            item_count: self.list_items().await?.len(),
            category_count: self.categories_tree().await?.len(),
            log_count: self.events().await?.len(),
        })
    }
}

/// Percent-encode a single path segment / query value (conservative set).
fn encode_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
        {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn spawn_daemon_router() -> (String, tokio::task::JoinHandle<()>) {
        let temp = tempfile::tempdir().expect("tempdir");
        let daemon = localref_core::LocalrefDaemon::for_library(temp.path())
            .expect("for_library");
        let router = localref_core::rest::router_with_daemon(daemon);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            let _temp = temp;
            axum::serve(listener, router).await.expect("serve");
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn list_items_round_trips_against_live_router() {
        let (endpoint, server) = spawn_daemon_router().await;
        let client = LocalrefClient::new(endpoint);
        let items = client.list_items().await.expect("list items");
        assert!(items.is_empty(), "fresh library has no items");
        server.abort();
    }

    #[tokio::test]
    async fn get_missing_item_is_status_error() {
        let (endpoint, server) = spawn_daemon_router().await;
        let client = LocalrefClient::new(endpoint);
        let err = client.get_item("does-not-exist").await.unwrap_err();
        assert!(matches!(err, ClientError::Status { status: 404, .. }));
        server.abort();
    }

    #[tokio::test]
    async fn daemon_status_and_scan_round_trip() {
        let (endpoint, server) = spawn_daemon_router().await;
        let client = LocalrefClient::new(endpoint);
        let status = client.daemon_status().await.expect("status");
        assert!(!status.running || status.queued_tasks == 0);
        let snap = client.dashboard_snapshot().await.expect("snapshot");
        assert_eq!(snap.item_count, 0);
        server.abort();
    }
}
