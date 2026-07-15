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
pub use localref_core::model::ScheduledCall;
use localref_core::model::{
    CategorySummary, ItemFilesDocument, MetadataDocument, SearchHit,
};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

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

/// Severity for a plugin-originated log entry (`POST /api/plugins/log`).
///
/// The host caps anything above `Warn`, so there is deliberately no `Error`
/// variant — a plugin cannot emit a host-level error record.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LogLevel {
    /// Most verbose; diagnostic tracing.
    Trace,
    /// Debug-level detail.
    Debug,
    /// Normal informational message.
    #[default]
    Info,
    /// Warning; something looked off but the plugin continued.
    Warn,
}

impl LogLevel {
    /// Wire string accepted by the log endpoint.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
        }
    }
}

/// Severity for a desktop notification (`POST /api/notify`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyKind {
    /// Informational notification.
    #[default]
    Info,
    /// Successful-operation notification.
    Success,
    /// Error notification.
    Error,
}

/// Async REST client bound to one daemon endpoint.
#[derive(Clone, Debug)]
pub struct LocalrefClient {
    /// REST endpoint without a trailing slash.
    endpoint: String,
    /// HTTP transport used for daemon requests.
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

    async fn delete_json<
        B: serde::Serialize,
        T: serde::de::DeserializeOwned,
    >(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let url = format!("{}{path}", self.endpoint);
        let resp = self
            .http
            .delete(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Self::decode(resp).await
    }

    /// POST a JSON body to a path, accepting a no-content response.
    ///
    /// Returns the HTTP status on any 2xx, plus `503 Service Unavailable` so
    /// callers can treat an unavailable capability (e.g. notifications in a
    /// headless build) as a soft outcome rather than an error. Other 4xx/5xx
    /// and transport failures map to `ClientError` like the JSON helpers.
    async fn post_unit<B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<reqwest::StatusCode, ClientError> {
        let url = format!("{}{path}", self.endpoint);
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status();
        if status.is_success()
            || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
        {
            return Ok(status);
        }
        let body = resp.text().await.unwrap_or_default();
        Err(ClientError::Status { status: status.as_u16(), body })
    }

    /// DELETE a path, returning the HTTP status on any 2xx.
    ///
    /// `404 Not Found` is returned as `Ok` so callers can treat "already gone"
    /// as a soft outcome; other 4xx/5xx and transport failures map to
    /// `ClientError`.
    async fn delete_unit(
        &self,
        path: &str,
    ) -> Result<reqwest::StatusCode, ClientError> {
        let url = format!("{}{path}", self.endpoint);
        let resp = self
            .http
            .delete(&url)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(status);
        }
        let body = resp.text().await.unwrap_or_default();
        Err(ClientError::Status { status: status.as_u16(), body })
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
        resp.json::<T>().await.map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// Return all indexed item documents.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot be reached or rejects the request.
    pub async fn list_items(&self) -> Result<Vec<ItemDocument>, ClientError> {
        self.get_json("/api/items").await
    }

    /// Return one item document by id.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot be reached or rejects the request.
    pub async fn get_item(
        &self,
        item_id: &str,
    ) -> Result<ItemDocument, ClientError> {
        self.get_json(&format!("/api/items/{}", encode_segment(item_id))).await
    }

    /// Return the files present in one item directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot be reached or rejects the request.
    pub async fn item_files(
        &self,
        item_id: &str,
    ) -> Result<ItemFilesDocument, ClientError> {
        self.get_json(&format!("/api/items/{}/files", encode_segment(item_id)))
            .await
    }

    /// Add an existing local file to an item, copying it into the item
    /// directory under a managed name and recording it in the item's metadata.
    ///
    /// `path` is a local filesystem path the daemon can read (absolute, or
    /// relative to the library root). Returns the reindexed item.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot add the file or rejects the request.
    pub async fn add_file(
        &self,
        item_id: &str,
        path: &str,
    ) -> Result<ItemDocument, ClientError> {
        self.post_json(
            &format!("/api/items/{}/files", encode_segment(item_id)),
            &serde_json::json!({ "path": path }),
        )
        .await
    }

    /// Add an existing local file at an exact item-relative path.
    pub async fn add_file_at(
        &self,
        item_id: &str,
        path: &str,
        relative_path: &str,
    ) -> Result<ItemDocument, ClientError> {
        self.post_json(
            &format!("/api/items/{}/files", encode_segment(item_id)),
            &serde_json::json!({ "path": path, "relative_path": relative_path }),
        )
        .await
    }

    /// Move an attachment into a recoverable plugin trash area.
    pub async fn archive_file(
        &self,
        item_id: &str,
        relative_path: &str,
        namespace: &str,
    ) -> Result<ItemDocument, ClientError> {
        self.delete_json(
            &format!("/api/items/{}/files", encode_segment(item_id)),
            &serde_json::json!({ "path": relative_path, "namespace": namespace }),
        )
        .await
    }

    /// Return the full metadata document for one item.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot be reached or rejects the request.
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
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot be reached or rejects the request.
    pub async fn categories_tree(
        &self,
    ) -> Result<Vec<CategorySummary>, ClientError> {
        self.get_json("/api/categories/tree").await
    }

    /// Add one item to one category.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot update the category or rejects the request.
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

    /// Set or clear one plugin `extra` value on an item.
    ///
    /// Pass `None` for `value` to remove the key. Returns the reindexed item.
    /// This is how a plugin persists its own per-item data; declaring the field
    /// as indexed in `plugin.toml` makes the value searchable.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot update the item or rejects the request.
    pub async fn set_item_extra(
        &self,
        item_id: &str,
        namespace: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<ItemDocument, ClientError> {
        self.post_json(
            &format!("/api/items/{}/extra", encode_segment(item_id)),
            &serde_json::json!({
                "namespace": namespace,
                "key": key,
                "value": value,
            }),
        )
        .await
    }

    /// Set (or clear, with `None`) the status-bar color shown on this item's
    /// row in the desktop library list.
    ///
    /// `color` is a CSS hex string like `"#e11d48"`; `None` removes it. This is
    /// a thin convenience over [`set_item_extra`](Self::set_item_extra) writing
    /// the reserved `ui.bar_color` extra, which the desktop app renders as a
    /// colored bar on the item's row (e.g. to flag a sync conflict).
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot update the item or rejects the request.
    pub async fn set_bar_color(
        &self,
        item_id: &str,
        color: Option<&str>,
    ) -> Result<ItemDocument, ClientError> {
        self.set_item_extra(item_id, "ui", "bar_color", color).await
    }

    /// Search indexed items by term.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot be reached or rejects the request.
    pub async fn search(
        &self,
        term: &str,
    ) -> Result<Vec<SearchHit>, ClientError> {
        self.get_json(&format!("/api/search?q={}", encode_segment(term))).await
    }

    /// Return daemon queue and pause status.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot be reached or rejects the request.
    pub async fn daemon_status(&self) -> Result<DaemonStatus, ClientError> {
        self.get_json("/api/daemon/status").await
    }

    /// Pause one daemon mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot change the pause mode or rejects the request.
    pub async fn pause(
        &self,
        mode: &str,
    ) -> Result<DaemonStatus, ClientError> {
        self.post_json(
            "/api/daemon/pause",
            &serde_json::json!({ "mode": mode }),
        )
        .await
    }

    /// Resume one daemon mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot change the pause mode or rejects the request.
    pub async fn resume(
        &self,
        mode: &str,
    ) -> Result<DaemonStatus, ClientError> {
        self.post_json(
            "/api/daemon/resume",
            &serde_json::json!({ "mode": mode }),
        )
        .await
    }

    /// Request a daemon scan.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot start the scan or rejects the request.
    pub async fn scan(&self) -> Result<serde_json::Value, ClientError> {
        self.post_json("/api/daemon/scan", &serde_json::Value::Null).await
    }

    /// Recent log entries from the ring buffer.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot be reached or rejects the request.
    pub async fn events(
        &self,
    ) -> Result<Vec<localref_core::logging::LogEntry>, ClientError> {
        self.get_json("/api/events").await
    }

    /// Aggregate dashboard counts (items, categories, recent events).
    ///
    /// # Errors
    ///
    /// Returns an error when any dashboard request fails.
    pub async fn dashboard_snapshot(
        &self,
    ) -> Result<DashboardSnapshot, ClientError> {
        Ok(DashboardSnapshot {
            item_count: self.list_items().await?.len(),
            category_count: self.categories_tree().await?.len(),
            log_count: self.events().await?.len(),
        })
    }

    /// Emit a log entry into the daemon's unified log under this plugin's name.
    ///
    /// The host records it under target `localref::plugin::<plugin>` and caps
    /// the level at `warn`.
    ///
    /// # Errors
    /// Returns an error on transport failure or a non-success HTTP status.
    pub async fn log(
        &self,
        plugin: &str,
        level: LogLevel,
        message: &str,
    ) -> Result<(), ClientError> {
        self.log_with(plugin, level, message, None, None, None).await
    }

    /// Emit a log entry with optional structured fields.
    ///
    /// `event_kind` is a stable identifier for the kind of event, `item_id` a
    /// related item, and `path` a library-relative path. Each is omitted from
    /// the record when `None`.
    ///
    /// # Errors
    /// Returns an error on transport failure or a non-success HTTP status.
    pub async fn log_with(
        &self,
        plugin: &str,
        level: LogLevel,
        message: &str,
        event_kind: Option<&str>,
        item_id: Option<&str>,
        path: Option<&str>,
    ) -> Result<(), ClientError> {
        let body = serde_json::json!({
            "plugin": plugin,
            "level": level.as_str(),
            "message": message,
            "event_kind": event_kind,
            "item_id": item_id,
            "path": path,
        });
        let _status = self.post_unit("/api/plugins/log", &body).await?;
        Ok(())
    }

    /// Request a desktop notification from the host.
    ///
    /// Notifications are best-effort: when the host has no notification
    /// capability (headless build, or notifications not started) the daemon
    /// responds `503` and this returns `Ok(false)` so the plugin can carry on.
    /// `Ok(true)` means the request was accepted for delivery.
    ///
    /// # Errors
    /// Returns an error on transport failure or a non-success HTTP status
    /// other than `503`.
    pub async fn notify(
        &self,
        title: &str,
        body: &str,
        kind: NotifyKind,
    ) -> Result<bool, ClientError> {
        let payload = serde_json::json!({
            "title": title,
            "body": body,
            "kind": kind,
        });
        let status = self.post_unit("/api/notify", &payload).await?;
        Ok(status != reqwest::StatusCode::SERVICE_UNAVAILABLE)
    }

    /// Push a status message to the desktop status bar.
    ///
    /// Best-effort like [`Self::notify`]: when the host has no UI subscribed
    /// (headless build or no live window) the message is simply dropped. The
    /// `kind` reuses [`NotifyKind`] and drives the status-bar indicator color.
    /// `Ok(true)` means the request was accepted; `Ok(false)` means the
    /// capability was unavailable (`503`).
    ///
    /// # Errors
    /// Returns an error on transport failure or a non-success HTTP status
    /// other than `503`.
    pub async fn set_status(
        &self,
        text: &str,
        kind: NotifyKind,
    ) -> Result<bool, ClientError> {
        let payload = serde_json::json!({
            "text": text,
            "kind": kind,
        });
        let status = self.post_unit("/api/status", &payload).await?;
        Ok(status != reqwest::StatusCode::SERVICE_UNAVAILABLE)
    }

    /// List all runtime-registered scheduled plugin calls.
    ///
    /// # Errors
    /// Returns an error on transport failure or a non-success HTTP status.
    pub async fn list_schedules(
        &self,
    ) -> Result<Vec<ScheduledCall>, ClientError> {
        self.get_json("/api/schedules").await
    }

    /// Register a scheduled plugin call (self or another plugin).
    ///
    /// # Errors
    /// Returns an error on transport failure or a non-success HTTP status; a
    /// duplicate id or invalid cron expression surfaces as a `400` status.
    pub async fn create_schedule(
        &self,
        call: &ScheduledCall,
    ) -> Result<(), ClientError> {
        let _ = self.post_unit("/api/schedules", call).await?;
        Ok(())
    }

    /// Remove a scheduled call by id.
    ///
    /// Returns `true` when a schedule was removed, `false` when none matched.
    /// # Errors
    /// Returns an error on transport failure or a non-success HTTP status other
    /// than `404`.
    pub async fn delete_schedule(
        &self,
        id: &str,
    ) -> Result<bool, ClientError> {
        let status = self
            .delete_unit(&format!("/api/schedules/{}", encode_segment(id)))
            .await?;
        Ok(status != reqwest::StatusCode::NOT_FOUND)
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
            write!(&mut out, "%{byte:02X}")
                .expect("writing to a String cannot fail");
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
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
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

    #[test]
    fn log_level_wire_strings() {
        assert_eq!(LogLevel::Trace.as_str(), "trace");
        assert_eq!(LogLevel::Debug.as_str(), "debug");
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Warn.as_str(), "warn");
        assert_eq!(LogLevel::default(), LogLevel::Info);
    }

    #[test]
    fn notify_kind_serializes_lowercase() {
        let json = serde_json::to_string(&NotifyKind::Success).unwrap();
        assert_eq!(json, "\"success\"");
    }

    #[tokio::test]
    async fn log_round_trips_against_live_router() {
        let (endpoint, server) = spawn_daemon_router().await;
        let client = LocalrefClient::new(endpoint);
        // The core router exposes /api/plugins/log; a successful call is Ok(()).
        client
            .log_with(
                "bibtexer",
                LogLevel::Info,
                "exported 2 items",
                Some("plugin_action"),
                Some("lr:zotero:abc"),
                None,
            )
            .await
            .expect("log accepted");
        server.abort();
    }

    /// Serve a fixed status at `/api/notify` for notify-path tests.
    async fn spawn_notify_router(
        status: axum::http::StatusCode,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let router = axum::Router::new().route(
            "/api/notify",
            axum::routing::post(move || async move { status }),
        );
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn notify_accepted_returns_true() {
        let (endpoint, server) =
            spawn_notify_router(axum::http::StatusCode::NO_CONTENT).await;
        let client = LocalrefClient::new(endpoint);
        let delivered = client
            .notify("Done", "Export complete", NotifyKind::Success)
            .await
            .expect("notify ok");
        assert!(delivered);
        server.abort();
    }

    #[tokio::test]
    async fn notify_unavailable_returns_false_not_error() {
        let (endpoint, server) =
            spawn_notify_router(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                .await;
        let client = LocalrefClient::new(endpoint);
        let delivered = client
            .notify("Done", "Export complete", NotifyKind::Info)
            .await
            .expect("503 is a soft outcome, not an error");
        assert!(!delivered);
        server.abort();
    }
}
