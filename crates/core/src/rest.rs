//! User-facing REST API for Localref.
//!
//! This crate exposes query-oriented API endpoints over the rebuildable storage
//! database. It does not write `All/` or `Cat/` directly; scan requests rebuild
//! the query cache from filesystem truth through `storage`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::model::Metadata;
use crate::rest_files;
use crate::storage::StorageDb;
use crate::types::CategoryPath;
use crate::{LocalrefDaemon, PauseMode, StatusKind};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Shared API application state.
#[derive(Clone)]
pub struct ApiState {
    /// Stored daemon.
    daemon: LocalrefDaemon,
}

/// API response returned by scan endpoints.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScanResponse {
    /// Number of item documents indexed after the scan.
    pub indexed_items: usize,
}

/// Request body for daemon pause and resume operations.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct PauseRequest {
    /// Pause mode to add or remove.
    pub mode: PauseMode,
}

/// Request body for metadata patch operations.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PatchMetadataRequest {
    /// Revision hash observed by the caller before editing.
    pub expected_revision: String,
    /// Complete replacement metadata document.
    pub metadata: Metadata,
}

/// Request body for setting one plugin `extra` value on an item.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct SetExtraRequest {
    /// Plugin namespace owning the field.
    pub namespace: String,
    /// Field key within the namespace.
    pub key: String,
    /// New value, or `null`/absent to remove the key.
    #[serde(default)]
    pub value: Option<String>,
}

/// Request body for importing an existing `All/<dir>` directory.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ImportFolderRequest {
    /// Absolute path or library-relative path to the directory.
    pub path: PathBuf,
}

/// Request body for opening one item-relative file path.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct OpenItemFileRequest {
    /// Path relative to the selected item directory.
    pub path: PathBuf,
}

/// Request body for adding one category to an item.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct CategoryRequest {
    /// Category path relative to `Cat/`.
    pub category: CategoryPath,
}

/// Request body for category rename and merge operations.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct CategoryMoveRequest {
    /// Source category path.
    pub from: CategoryPath,
    /// Destination category path.
    pub to: CategoryPath,
}

/// Request body for a plugin-originated log entry.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct PluginLogRequest {
    /// Originating plugin name (sanitized into the log target).
    pub plugin: String,
    /// Requested level: `trace`, `debug`, `info`, or `warn`. Anything higher
    /// (or unrecognized) is capped to `warn` so a plugin cannot forge `error`.
    pub level: String,
    /// Human-readable log message.
    pub message: String,
    /// Optional stable event-kind identifier.
    #[serde(default)]
    pub event_kind: Option<String>,
    /// Optional related item identifier.
    #[serde(default)]
    pub item_id: Option<String>,
    /// Optional library-relative path.
    #[serde(default)]
    pub path: Option<String>,
}

/// Request body for a plugin-pushed desktop status-bar message.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct StatusRequest {
    /// Message text to show in the status bar.
    pub text: String,
    /// Severity driving the status-bar indicator color; defaults to `info`.
    #[serde(default)]
    pub kind: StatusKind,
}

/// Build the user-facing Localref API router.
pub fn router(storage: StorageDb) -> Router {
    router_with_daemon(LocalrefDaemon::new(storage))
}

/// Build the user-facing Localref API router with an existing daemon facade.
pub fn router_with_daemon(daemon: LocalrefDaemon) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/daemon/status", get(daemon_status))
        .route("/api/daemon/pause", post(pause))
        .route("/api/daemon/resume", post(resume))
        .route("/api/daemon/scan", post(scan))
        .route("/api/events", get(events))
        .route("/api/events/stream", get(events_stream))
        .route("/api/categories/tree", get(categories_tree))
        .route("/api/categories", post(create_category))
        .route("/api/items", get(list_items))
        .route("/api/items/{id}", get(get_item))
        .route("/api/items/{id}/files", get(item_files).post(add_item_file))
        .route("/api/items/{id}/files/open", post(open_item_file))
        .route("/api/items/{id}/folder/open", post(open_item_folder))
        .route(
            "/api/items/{id}/metadata",
            get(get_metadata).patch(patch_metadata),
        )
        .route("/api/items/{id}/extra", post(set_item_extra))
        .route("/api/items/{id}/categories", post(add_item_category))
        .route(
            "/api/items/{id}/categories/{*category}",
            delete(remove_item_category),
        )
        .route("/api/categories/rename", post(rename_category))
        .route("/api/categories/merge", post(merge_category))
        .route("/api/import/folder", post(import_folder))
        .route("/api/import/file", post(import_file))
        .route("/api/import/cat-folder", post(normalize_cat_folder))
        .route("/api/search", get(search))
        .route("/api/plugins/log", post(plugin_log))
        .route("/api/status", post(set_status))
        .route("/api/schedules", get(list_schedules).post(create_schedule))
        .route("/api/schedules/{id}", delete(delete_schedule))
        .with_state(ApiState { daemon })
}

/// Open storage at `library_root` and build the API router.
/// # Errors
///
/// Returns an error when the operation cannot be completed.
pub fn router_for_library(
    library_root: impl Into<PathBuf>,
) -> crate::error::Result<Router> {
    Ok(router(StorageDb::open(library_root)?))
}

/// Run the user-facing API server until the process is stopped.
/// # Errors
///
/// Returns an error when the operation cannot be completed.
pub async fn serve(
    addr: SocketAddr,
    storage: StorageDb,
) -> std::io::Result<()> {
    serve_with_daemon(addr, LocalrefDaemon::new(storage)).await
}

/// Run the user-facing API server with an existing daemon facade.
/// # Errors
///
/// Returns an error when the operation cannot be completed.
pub async fn serve_with_daemon(
    addr: SocketAddr,
    daemon: LocalrefDaemon,
) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router_with_daemon(daemon)).await
}

/// Internal helper for health.
pub async fn health() -> Response {
    Json(json!({
        "status": "ok",
        "service": "localref-rest"
    }))
    .into_response()
}

/// Record a plugin-originated log entry in the unified log.
///
/// The plugin name is sanitized into a per-plugin target
/// (`localref::plugin::<name>`) and the level is capped at `WARN` so a plugin
/// cannot emit `error`-level records that imply a host failure. Always returns
/// `204 No Content`.
pub async fn plugin_log(Json(request): Json<PluginLogRequest>) -> Response {
    let target =
        format!("localref::plugin::{}", sanitize_plugin(&request.plugin));
    let level = capped_level(&request.level);
    crate::logging::log_dynamic(
        &target,
        level,
        &request.message,
        request.event_kind.as_deref(),
        request.item_id.as_deref(),
        request.path.as_deref(),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// Push a plugin status message to the desktop status bar.
///
/// Publishes a [`DaemonEvent::StatusMessage`](crate::DaemonEvent) so the
/// subscribed UI updates its status bar, and mirrors the text to the unified
/// log so it is also visible there. Always returns `204 No Content`.
pub async fn set_status(
    State(state): State<ApiState>,
    Json(request): Json<StatusRequest>,
) -> Response {
    tracing::info!(
        target: "localref::status",
        kind = ?request.kind,
        "{}",
        request.text,
    );
    state.daemon.emit_status(request.text, request.kind);
    StatusCode::NO_CONTENT.into_response()
}

/// List all runtime-registered scheduled plugin calls.
pub async fn list_schedules(State(state): State<ApiState>) -> Response {
    match state.daemon.list_schedules() {
        Ok(schedules) => Json(schedules).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Register a scheduled plugin call.
///
/// Returns `400 Bad Request` for a duplicate id or invalid cron expression,
/// `500` for storage failures, and `201 Created` on success.
pub async fn create_schedule(
    State(state): State<ApiState>,
    Json(call): Json<crate::schedule::ScheduledCall>,
) -> Response {
    match state.daemon.register_schedule(call) {
        Ok(()) => StatusCode::CREATED.into_response(),
        Err(crate::error::LocalrefError::Rule(message)) => {
            api_error(StatusCode::BAD_REQUEST, message)
        }
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Remove a scheduled call by id.
///
/// Returns `404 Not Found` when no schedule matched, `204 No Content` on
/// success.
pub async fn delete_schedule(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Response {
    match state.daemon.remove_schedule(&id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => api_error(StatusCode::NOT_FOUND, "no such schedule"),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

///
/// Keeps log targets predictable and prevents a plugin from injecting `::`
/// segments or whitespace into the target path. Empty input becomes
/// `unknown`.
// Single caller (`plugin_log`); kept separate for direct unit testing.
#[allow(clippy::single_call_fn)]
fn sanitize_plugin(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if slug.is_empty() { "unknown".to_string() } else { slug }
}

/// Parse a wire level string, capping anything above `WARN` to `WARN`.
// Single caller (`plugin_log`); kept separate for direct unit testing.
#[allow(clippy::single_call_fn)]
fn capped_level(level: &str) -> tracing::Level {
    match level.to_ascii_lowercase().as_str() {
        "trace" => tracing::Level::TRACE,
        "debug" => tracing::Level::DEBUG,
        "info" => tracing::Level::INFO,
        // "warn", "error", and anything unrecognized all cap at WARN.
        _ => tracing::Level::WARN,
    }
}

/// Internal helper for scan.
pub async fn scan(State(state): State<ApiState>) -> Response {
    match state.daemon.scan_all() {
        Ok(task) => Json(task).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for daemon status.
pub async fn daemon_status(State(state): State<ApiState>) -> Response {
    Json(state.daemon.status()).into_response()
}

/// Internal helper for pause.
pub async fn pause(
    State(state): State<ApiState>,
    Json(request): Json<PauseRequest>,
) -> Response {
    Json(state.daemon.pause(request.mode)).into_response()
}

/// Internal helper for resume.
pub async fn resume(
    State(state): State<ApiState>,
    Json(request): Json<PauseRequest>,
) -> Response {
    Json(state.daemon.resume(request.mode)).into_response()
}

/// Internal helper for list items.
pub async fn list_items(State(state): State<ApiState>) -> Response {
    match state.daemon.list_items() {
        Ok(items) => Json(items).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for get item.
pub async fn get_item(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Response {
    match state.daemon.get_item(&id) {
        Ok(Some(item)) => Json(item).into_response(),
        Ok(None) => {
            api_error(StatusCode::NOT_FOUND, format!("item not found: {id}"))
        }
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for item files.
pub async fn item_files(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Response {
    match rest_files::item_files(&state.daemon, &id) {
        Ok(Some(files)) => Json(files).into_response(),
        Ok(None) => {
            api_error(StatusCode::NOT_FOUND, format!("item not found: {id}"))
        }
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for open item file.
pub async fn open_item_file(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<OpenItemFileRequest>,
) -> Response {
    match rest_files::item_file_path(&state.daemon, &id, &request.path) {
        Ok(Some(path)) => match rest_files::open_system_path(&path) {
            Ok(()) => Json(json!({"opened": request.path})).into_response(),
            Err(error) => {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            }
        },
        Ok(None) => api_error(
            StatusCode::NOT_FOUND,
            format!("item file not found: {id}"),
        ),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error.to_string()),
    }
}

/// Internal helper for add item file.
pub async fn add_item_file(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<ImportFolderRequest>,
) -> Response {
    match state.daemon.add_file_to_item(&id, request.path) {
        Ok(item) => Json(item).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for open item folder.
pub async fn open_item_folder(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Response {
    match rest_files::item_folder(&state.daemon, &id) {
        Ok(Some(path)) => match rest_files::open_system_path(&path) {
            Ok(()) => Json(json!({"opened": id})).into_response(),
            Err(error) => {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            }
        },
        Ok(None) => {
            api_error(StatusCode::NOT_FOUND, format!("item not found: {id}"))
        }
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Return the full metadata document and source revision for one item.
pub async fn get_metadata(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Response {
    match state.daemon.get_metadata(&id) {
        Ok(Some(document)) => Json(document).into_response(),
        Ok(None) => {
            api_error(StatusCode::NOT_FOUND, format!("item not found: {id}"))
        }
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for patch metadata.
pub async fn patch_metadata(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<PatchMetadataRequest>,
) -> Response {
    match state.daemon.patch_metadata(
        &id,
        &request.expected_revision,
        &request.metadata,
    ) {
        Ok(item) => Json(item).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Set or clear one plugin `extra` value on an item.
pub async fn set_item_extra(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<SetExtraRequest>,
) -> Response {
    match state.daemon.set_item_extra(
        &id,
        &request.namespace,
        &request.key,
        request.value.as_deref(),
    ) {
        Ok(item) => Json(item).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for import folder.
pub async fn import_folder(
    State(state): State<ApiState>,
    Json(request): Json<ImportFolderRequest>,
) -> Response {
    match state.daemon.import_all_directory(request.path) {
        Ok(outcome) => Json(outcome).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for import file.
pub async fn import_file(
    State(state): State<ApiState>,
    Json(request): Json<ImportFolderRequest>,
) -> Response {
    match state.daemon.import_file(request.path) {
        Ok(outcome) => Json(outcome).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for normalize cat folder.
pub async fn normalize_cat_folder(
    State(state): State<ApiState>,
    Json(request): Json<ImportFolderRequest>,
) -> Response {
    match state.daemon.normalize_cat_directory(request.path) {
        Ok(outcome) => Json(outcome).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for add item category.
pub async fn add_item_category(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<CategoryRequest>,
) -> Response {
    match state.daemon.add_item_category(&id, &request.category) {
        Ok(summary) => Json(summary).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for create category.
pub async fn create_category(
    State(state): State<ApiState>,
    Json(request): Json<CategoryRequest>,
) -> Response {
    match state.daemon.create_category(&request.category) {
        Ok(summary) => Json(summary).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for remove item category.
pub async fn remove_item_category(
    State(state): State<ApiState>,
    Path((id, category)): Path<(String, String)>,
) -> Response {
    let Some(category) = CategoryPath::new(category) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid category path");
    };
    match state.daemon.remove_item_category(&id, &category) {
        Ok(summary) => Json(summary).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for rename category.
pub async fn rename_category(
    State(state): State<ApiState>,
    Json(request): Json<CategoryMoveRequest>,
) -> Response {
    match state.daemon.rename_category(&request.from, &request.to) {
        Ok(summary) => Json(summary).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for merge category.
pub async fn merge_category(
    State(state): State<ApiState>,
    Json(request): Json<CategoryMoveRequest>,
) -> Response {
    match state.daemon.merge_category(&request.from, &request.to) {
        Ok(summary) => Json(summary).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for search.
pub async fn search(
    State(state): State<ApiState>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    let Some(term) = query.get("q") else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "missing query parameter: q",
        );
    };
    match state.daemon.search(term) {
        Ok(hits) => Json(hits).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for events.
pub async fn events(State(state): State<ApiState>) -> Response {
    match state.daemon.events() {
        Ok(events) => Json(events).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for events stream.
pub async fn events_stream(State(state): State<ApiState>) -> Response {
    match state.daemon.events() {
        Ok(events) => {
            use std::fmt::Write as _;

            let body =
                events.into_iter().fold(String::new(), |mut body, event| {
                    let event_name =
                        event.event_kind.as_deref().unwrap_or("log");
                    let json =
                        serde_json::to_string(&event).unwrap_or_default();
                    let _ =
                        write!(body, "event: {event_name}\ndata: {json}\n\n");
                    body
                });
            let mut response = body.into_response();
            response.headers_mut().insert(
                CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            response
        }
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for categories tree.
pub async fn categories_tree(State(state): State<ApiState>) -> Response {
    match state.daemon.list_categories() {
        Ok(categories) => Json(categories).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

/// Internal helper for api error.
fn api_error(status: StatusCode, message: impl Into<String>) -> Response {
    let mut response =
        Json(json!({ "error": message.into() })).into_response();
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ConnectorImport, ConnectorItem};
    use axum::body::Body;
    use axum::body::to_bytes;
    use http::Request;
    use serde_json::Value;
    use tower::ServiceExt;

    #[tokio::test]
    async fn scans_lists_and_searches_items() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Paper One");
        std::fs::create_dir_all(&item_dir).unwrap();
        std::fs::write(
            item_dir.join("metadata.toml"),
            r#"
id = "lr:test:1"
type = "journalArticle"
title = "Near Field RIS Paper"
abstract = "REST-visible abstract text"
doi = "10.1234/example"

[[creators]]
role = "author"
name = "Near Field Author"

[files]
main = "paper.pdf"
"#,
        )
        .unwrap();

        let app = router_for_library(temp.path()).unwrap();
        let scan = request_json(&app, "POST", "/api/daemon/scan").await;
        assert_eq!(scan["state"], "completed");
        assert_eq!(scan["indexed_items"], 1);

        let status = request_json(&app, "GET", "/api/daemon/status").await;
        assert_eq!(status["recent_tasks"][0]["task"], "scan_all");

        let items = request_json(&app, "GET", "/api/items").await;
        assert_eq!(items[0]["id"], "lr:test:1");
        assert_eq!(items[0]["abstract_note"], "REST-visible abstract text");
        assert_eq!(items[0]["authors"][0], "Near Field Author");

        let hits = request_json(&app, "GET", "/api/search?q=ris").await;
        assert_eq!(hits[0]["title"], "Near Field RIS Paper");
        assert_eq!(hits[0]["authors"][0], "Near Field Author");
        let abstract_hits =
            request_json(&app, "GET", "/api/search?q=visible").await;
        assert_eq!(abstract_hits[0]["id"], "lr:test:1");
        let author_hits =
            request_json(&app, "GET", "/api/search?q=author").await;
        assert_eq!(author_hits[0]["id"], "lr:test:1");
    }

    #[tokio::test]
    async fn pauses_and_resumes_indexing() {
        let temp = tempfile::tempdir().unwrap();
        let app = router_for_library(temp.path()).unwrap();

        let paused = request_json_body(
            &app,
            "POST",
            "/api/daemon/pause",
            json!({"mode": "indexing"}),
        )
        .await;
        assert_eq!(paused["paused_modes"][0], "indexing");

        let scan_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/daemon/scan")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(scan_response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let resumed = request_json_body(
            &app,
            "POST",
            "/api/daemon/resume",
            json!({"mode": "indexing"}),
        )
        .await;
        assert!(resumed["paused_modes"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn returns_categories_derived_from_cat_links() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".localref")).unwrap();
        std::fs::write(
            temp.path().join(".localref").join("rules.toml"),
            r#"
[[rules]]
name = "rest-category"
target = "Wireless/RIS"
query = 'title:RIS'
"#,
        )
        .unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-rest-cat".to_string()),
                    uri: None,
                    connector_item_id: Some("rest-cat".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "RIS Category Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "RIS Category Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();
        let app = router_with_daemon(daemon);

        let categories =
            request_json(&app, "GET", "/api/categories/tree").await;

        assert_eq!(categories[0]["path"], "Wireless/RIS");
        assert_eq!(categories[0]["item_ids"][0], "lr:zotero:rest-cat");
    }

    #[tokio::test]
    async fn creates_empty_category_from_api() {
        let temp = tempfile::tempdir().unwrap();
        let app = router_for_library(temp.path()).unwrap();

        let created = request_json_body(
            &app,
            "POST",
            "/api/categories",
            json!({"category": "Inbox/New"}),
        )
        .await;

        assert_eq!(created["path"], "Inbox/New");
        assert!(created["item_ids"].as_array().unwrap().is_empty());
        assert!(temp.path().join("Cat").join("Inbox").join("New").is_dir());
        let categories =
            request_json(&app, "GET", "/api/categories/tree").await;
        assert!(
            categories
                .as_array()
                .unwrap()
                .iter()
                .any(|category| category["path"] == "Inbox/New")
        );
    }

    #[tokio::test]
    async fn patches_metadata_with_revision() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Patch Paper");
        std::fs::create_dir_all(&item_dir).unwrap();
        let metadata_text = r#"
id = "lr:test:patch"
type = "journalArticle"
title = "Patch Paper"

[files]
main = "paper.pdf"
"#;
        std::fs::write(item_dir.join("metadata.toml"), metadata_text).unwrap();
        let app = router_for_library(temp.path()).unwrap();
        request_json(&app, "POST", "/api/daemon/scan").await;
        let item = request_json(&app, "GET", "/api/items/lr:test:patch").await;
        let metadata_document =
            request_json(&app, "GET", "/api/items/lr:test:patch/metadata")
                .await;
        assert_eq!(metadata_document["metadata"]["title"], "Patch Paper");
        assert_eq!(
            metadata_document["metadata_revision"],
            item["metadata_revision"]
        );
        let mut metadata = Metadata::from_toml_str(metadata_text).unwrap();
        metadata.title = "REST Patched Paper".to_string();

        let patched = request_json_body(
            &app,
            "PATCH",
            "/api/items/lr:test:patch/metadata",
            json!({
                "expected_revision": item["metadata_revision"],
                "metadata": metadata
            }),
        )
        .await;

        assert_eq!(patched["title"], "REST Patched Paper");
        assert_ne!(patched["metadata_revision"], item["metadata_revision"]);
    }

    #[tokio::test]
    async fn imports_existing_all_folder() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("ManualRESTPaper");
        std::fs::create_dir_all(&item_dir).unwrap();
        std::fs::write(item_dir.join("paper.pdf"), b"pdf").unwrap();
        let app = router_for_library(temp.path()).unwrap();

        let outcome = request_json_body(
            &app,
            "POST",
            "/api/import/folder",
            json!({"path": "All/ManualRESTPaper"}),
        )
        .await;

        assert_eq!(outcome["item_id"], "lr:manual:ManualRESTPaper");
        let item =
            request_json(&app, "GET", "/api/items/lr:manual:ManualRESTPaper")
                .await;
        assert_eq!(item["main_file"], "paper.pdf");
    }

    #[tokio::test]
    async fn imports_explicit_file_and_streams_events() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("paper.pdf");
        std::fs::write(&source, b"pdf").unwrap();
        let app = router_for_library(temp.path()).unwrap();

        let outcome = request_json_body(
            &app,
            "POST",
            "/api/import/file",
            json!({"path": source}),
        )
        .await;
        assert_eq!(outcome["item_id"], "lr:manual:paper");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/events/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["content-type"], "text/event-stream");
    }

    #[tokio::test]
    async fn lists_item_folder_files_without_exposing_parent_paths() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("File Paper");
        std::fs::create_dir_all(item_dir.join("figures")).unwrap();
        std::fs::write(item_dir.join("paper.pdf"), b"pdf").unwrap();
        std::fs::write(item_dir.join("figures").join("one.png"), b"png")
            .unwrap();
        std::fs::write(
            item_dir.join("metadata.toml"),
            r#"
id = "lr:test:files"
type = "journalArticle"
title = "File Paper"

[files]
main = "paper.pdf"
"#,
        )
        .unwrap();
        let app = router_for_library(temp.path()).unwrap();
        request_json(&app, "POST", "/api/daemon/scan").await;

        let files =
            request_json(&app, "GET", "/api/items/lr:test:files/files").await;

        assert_eq!(files["item_id"], "lr:test:files");
        assert!(
            files["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == "paper.pdf"
                    && entry["kind"] == "file")
        );
        assert!(
            files["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == "figures/one.png")
        );

        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/items/lr:test:files/files/open")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"path": "../paper.pdf"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn adds_dropped_file_to_existing_item_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Drop Target");
        std::fs::create_dir_all(&item_dir).unwrap();
        std::fs::write(
            item_dir.join("metadata.toml"),
            r#"
id = "lr:test:drop"
type = "journalArticle"
title = "Drop Target"
"#,
        )
        .unwrap();
        let source = temp.path().join("appendix.pdf");
        std::fs::write(&source, b"pdf").unwrap();
        let app = router_for_library(temp.path()).unwrap();
        request_json(&app, "POST", "/api/daemon/scan").await;

        let item = request_json_body(
            &app,
            "POST",
            "/api/items/lr:test:drop/files",
            json!({"path": source}),
        )
        .await;

        assert_eq!(item["main_file"], "appendix.pdf");
        assert!(item_dir.join("appendix.pdf").is_file());
    }

    #[test]
    fn unmatched_connector_import_links_under_unmatched_category() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let outcome = daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("s-unmatched".to_string()),
                    uri: None,
                    connector_item_id: Some("unmatched-1".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Totally Unclassifiable Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Totally Unclassifiable Paper"}),
                },
                attachments: Vec::new(),
            })
            .expect("import should succeed");

        assert!(outcome.item_dir.exists(), "item dir written to All/");
        assert_eq!(
            outcome.categories,
            vec![CategoryPath::new("unmatched").unwrap()],
            "unmatched import must be classified as 'unmatched'",
        );
        // The item must be reachable from Cat/unmatched/, not merely have the
        // directory created — that is the orphan-prevention guarantee.
        let unmatched_dir = temp.path().join("Cat").join("unmatched");
        let link_count = std::fs::read_dir(&unmatched_dir)
            .expect("Cat/unmatched/ must exist after an unmatched import")
            .count();
        assert_eq!(
            link_count, 1,
            "Cat/unmatched/ must contain exactly one link to the imported item",
        );
    }

    #[test]
    fn filing_unmatched_item_into_real_category_removes_unmatched() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("s".to_string()),
                    uri: None,
                    connector_item_id: Some("refile-1".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Refile Me".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Refile Me"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();
        // Sanity: it starts in unmatched.
        let before = daemon.get_item("lr:zotero:refile-1").unwrap().unwrap();
        assert_eq!(before.categories, vec!["unmatched"]);

        daemon.create_category(CategoryPath::new("Inbox").unwrap()).unwrap();
        daemon
            .add_item_category(
                "lr:zotero:refile-1",
                CategoryPath::new("Inbox").unwrap(),
            )
            .unwrap();

        let after = daemon.get_item("lr:zotero:refile-1").unwrap().unwrap();
        assert_eq!(
            after.categories,
            vec!["Inbox"],
            "filing an unmatched item into a real category must drop 'unmatched'",
        );
    }

    #[tokio::test]
    async fn category_write_endpoints_update_cat_links() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-rest-write-cat".to_string()),
                    uri: None,
                    connector_item_id: Some("rest-write-cat".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "REST Category Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "REST Category Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();
        let app = router_with_daemon(daemon);

        let added = request_json_body(
            &app,
            "POST",
            "/api/items/lr:zotero:rest-write-cat/categories",
            json!({"category": "Wireless/RIS"}),
        )
        .await;
        assert_eq!(added["path"], "Wireless/RIS");

        let renamed = request_json_body(
            &app,
            "POST",
            "/api/categories/rename",
            json!({"from": "Wireless/RIS", "to": "Wireless/NearField"}),
        )
        .await;
        assert_eq!(renamed["path"], "Wireless/NearField");

        let merged = request_json_body(
            &app,
            "POST",
            "/api/categories/merge",
            json!({"from": "Wireless/NearField", "to": "Archive"}),
        )
        .await;
        assert_eq!(merged["path"], "Archive");

        let removed = request_json(
            &app,
            "DELETE",
            "/api/items/lr:zotero:rest-write-cat/categories/Archive",
        )
        .await;
        assert!(removed["item_ids"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn normalizes_real_cat_folder() {
        let temp = tempfile::tempdir().unwrap();
        let cat_dir = temp.path().join("Cat").join("Inbox").join("Copied");
        std::fs::create_dir_all(&cat_dir).unwrap();
        std::fs::write(
            cat_dir.join("metadata.toml"),
            r#"
id = "lr:manual:Copied"
type = "document"
title = "Copied"
"#,
        )
        .unwrap();
        let app = router_for_library(temp.path()).unwrap();

        let outcome = request_json_body(
            &app,
            "POST",
            "/api/import/cat-folder",
            json!({"path": "Cat/Inbox/Copied"}),
        )
        .await;

        assert_eq!(outcome["item_id"], "lr:manual:Copied");
        let categories =
            request_json(&app, "GET", "/api/categories/tree").await;
        assert_eq!(categories[0]["path"], "Inbox");
    }

    #[test]
    fn scan_replaces_empty_stale_item_directory_with_link() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Paper");
        std::fs::create_dir_all(&item_dir).unwrap();
        std::fs::write(
            item_dir.join("metadata.toml"),
            r#"
id = "lr:manual:paper"
type = "document"
title = "Paper"
"#,
        )
        .unwrap();
        let stale_dir = temp.path().join("Cat").join("Inbox").join("Paper");
        std::fs::create_dir_all(&stale_dir).unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        daemon.scan_all().unwrap();

        assert_eq!(
            stale_dir.canonicalize().unwrap(),
            item_dir.canonicalize().unwrap(),
            "a stale empty item directory must become a link to its All item",
        );
        let item = daemon.get_item("lr:manual:paper").unwrap().unwrap();
        assert_eq!(item.categories, vec!["Inbox"]);
    }

    #[test]
    fn normalization_uses_metadata_id_to_link_an_existing_all_item() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Canonical");
        std::fs::create_dir_all(&item_dir).unwrap();
        std::fs::write(
            item_dir.join("metadata.toml"),
            r#"
id = "lr:manual:same"
type = "document"
title = "Canonical"
"#,
        )
        .unwrap();
        let cat_dir = temp.path().join("Cat").join("Inbox").join("Copied");
        std::fs::create_dir_all(&cat_dir).unwrap();
        std::fs::write(
            cat_dir.join("metadata.toml"),
            r#"
id = "lr:manual:same"
type = "document"
title = "Different copy"
"#,
        )
        .unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        daemon.normalize_cat_directory(&cat_dir).unwrap();

        assert_eq!(
            cat_dir.canonicalize().unwrap(),
            item_dir.canonicalize().unwrap(),
            "matching metadata IDs must link to the existing All item",
        );
        let metadata =
            std::fs::read_to_string(item_dir.join("metadata.toml")).unwrap();
        assert!(metadata.contains("Canonical"));
        assert!(!metadata.contains("Different copy"));
    }

    #[test]
    fn normalization_keeps_same_named_items_with_different_ids_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let existing_dir = temp.path().join("All").join("Paper");
        std::fs::create_dir_all(&existing_dir).unwrap();
        std::fs::write(
            existing_dir.join("metadata.toml"),
            r#"
id = "lr:manual:existing"
type = "document"
title = "Existing"
"#,
        )
        .unwrap();
        let cat_dir = temp.path().join("Cat").join("Inbox").join("Paper");
        std::fs::create_dir_all(&cat_dir).unwrap();
        std::fs::write(
            cat_dir.join("metadata.toml"),
            r#"
id = "lr:manual:distinct"
type = "document"
title = "Distinct"
"#,
        )
        .unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        let outcome = daemon.normalize_cat_directory(&cat_dir).unwrap();

        assert_ne!(outcome.item_dir, existing_dir);
        assert!(existing_dir.join("metadata.toml").is_file());
        assert_eq!(
            cat_dir.canonicalize().unwrap(),
            outcome.item_dir.canonicalize().unwrap(),
            "different metadata IDs must remain distinct All items",
        );
    }

    async fn request_json(app: &Router, method: &str, uri: &str) -> Value {
        request_json_body(app, method, uri, Value::Null).await
    }

    /// Send a request and return only the response status (no success assert).
    async fn request_status(
        app: &Router,
        method: &str,
        uri: &str,
        body: Value,
    ) -> StatusCode {
        let body = if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        };
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn schedules_crud_validates_and_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let app = router_for_library(temp.path()).unwrap();

        // Empty list on a fresh library.
        let empty = request_json(&app, "GET", "/api/schedules").await;
        assert_eq!(empty.as_array().unwrap().len(), 0);

        // Register a valid schedule.
        let created = request_status(
            &app,
            "POST",
            "/api/schedules",
            json!({
                "id": "nightly",
                "plugin": "archiver",
                "action": "backup",
                "params": {"format": "bibtex"},
                "schedule": "0 0 3 * * *"
            }),
        )
        .await;
        assert_eq!(created, StatusCode::CREATED);

        // It is listed back with its params.
        let listed = request_json(&app, "GET", "/api/schedules").await;
        assert_eq!(listed[0]["id"], "nightly");
        assert_eq!(listed[0]["params"]["format"], "bibtex");

        // A duplicate id is rejected with 400.
        let duplicate = request_status(
            &app,
            "POST",
            "/api/schedules",
            json!({
                "id": "nightly",
                "plugin": "archiver",
                "action": "backup",
                "schedule": "0 0 4 * * *"
            }),
        )
        .await;
        assert_eq!(duplicate, StatusCode::BAD_REQUEST);

        // An invalid cron expression is rejected with 400.
        let invalid = request_status(
            &app,
            "POST",
            "/api/schedules",
            json!({
                "id": "broken",
                "plugin": "archiver",
                "action": "backup",
                "schedule": "not a cron expr"
            }),
        )
        .await;
        assert_eq!(invalid, StatusCode::BAD_REQUEST);

        // Delete the schedule, then a second delete reports 404.
        let deleted = request_status(
            &app,
            "DELETE",
            "/api/schedules/nightly",
            Value::Null,
        )
        .await;
        assert_eq!(deleted, StatusCode::NO_CONTENT);
        let missing = request_status(
            &app,
            "DELETE",
            "/api/schedules/nightly",
            Value::Null,
        )
        .await;
        assert_eq!(missing, StatusCode::NOT_FOUND);
    }

    #[test]
    fn sanitize_plugin_slugs_unsafe_chars() {
        assert_eq!(sanitize_plugin("BibTexer"), "bibtexer");
        assert_eq!(sanitize_plugin("my plugin::x"), "my_plugin__x");
        assert_eq!(sanitize_plugin("keep-_09"), "keep-_09");
        assert_eq!(sanitize_plugin(""), "unknown");
    }

    #[test]
    fn capped_level_never_exceeds_warn() {
        assert_eq!(capped_level("trace"), tracing::Level::TRACE);
        assert_eq!(capped_level("INFO"), tracing::Level::INFO);
        assert_eq!(capped_level("warn"), tracing::Level::WARN);
        // error and unknown both cap to WARN.
        assert_eq!(capped_level("error"), tracing::Level::WARN);
        assert_eq!(capped_level("nonsense"), tracing::Level::WARN);
    }

    #[tokio::test]
    async fn plugin_log_returns_no_content() {
        let temp = tempfile::tempdir().unwrap();
        let app = router_for_library(temp.path()).unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/plugins/log")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "plugin": "bibtexer",
                            "level": "info",
                            "message": "exported 3 items",
                            "item_id": "lr:zotero:abc"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn set_status_returns_no_content() {
        let temp = tempfile::tempdir().unwrap();
        let app = router_for_library(temp.path()).unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/status")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "text": "syncing", "kind": "error" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    async fn request_json_body(
        app: &Router,
        method: &str,
        uri: &str,
        body: Value,
    ) -> Value {
        let body = if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success(), "{}", response.status());
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Write a minimal managed item under `All/` and return its directory.
    fn write_item(root: &std::path::Path, id: &str, title: &str) -> PathBuf {
        let item_dir = root.join("All").join(title);
        std::fs::create_dir_all(&item_dir).unwrap();
        std::fs::write(
            item_dir.join("metadata.toml"),
            format!("id = \"{id}\"\ntype = \"document\"\ntitle = \"{title}\"\n"),
        )
        .unwrap();
        item_dir
    }

    #[test]
    fn deleting_a_junction_then_scanning_reprojects_it_from_metadata() {
        // metadata.toml is the source of truth: deleting a Cat/ junction in a
        // file manager is NOT a removal. The next scan re-projects the junction
        // from metadata; the category stays and no tombstone is written.
        // (Removal is metadata/UI/API only.)
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        write_item(temp.path(), "lr:test:drop", "Paper");
        daemon.scan_all().unwrap();
        daemon
            .add_item_category(
                "lr:test:drop",
                CategoryPath::new("Inbox").unwrap(),
            )
            .unwrap();
        assert_eq!(
            daemon.get_item("lr:test:drop").unwrap().unwrap().categories,
            vec!["Inbox"],
        );

        // Simulate the user deleting the junction in their file manager.
        let junction = temp.path().join("Cat").join("Inbox").join("Paper");
        std::fs::remove_dir(&junction).unwrap();
        daemon.scan_all().unwrap();

        let item = daemon.get_item("lr:test:drop").unwrap().unwrap();
        assert_eq!(
            item.categories,
            vec!["Inbox"],
            "a deleted junction must be re-projected, not treated as a removal",
        );
        assert!(
            junction.exists(),
            "the junction must be re-created from metadata",
        );
        let metadata = std::fs::read_to_string(
            temp.path().join("All").join("Paper").join("metadata.toml"),
        )
        .unwrap();
        let parsed = Metadata::from_toml_str(&metadata).unwrap();
        assert!(
            parsed.state.removed_categories.is_empty(),
            "re-projection must not write a spurious tombstone",
        );
    }

    #[test]
    fn editing_metadata_to_drop_a_category_removes_the_junction() {
        // Removal happens through metadata: when a projected category is dropped
        // from metadata.toml, the next scan removes the now-orphaned junction.
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        write_item(temp.path(), "lr:test:edit", "Paper");
        daemon.scan_all().unwrap();
        daemon
            .add_item_category(
                "lr:test:edit",
                CategoryPath::new("Inbox").unwrap(),
            )
            .unwrap();
        let junction = temp.path().join("Cat").join("Inbox").join("Paper");
        assert!(junction.exists(), "precondition: junction projected");

        // The user edits metadata.toml directly, removing the category line.
        std::fs::write(
            temp.path().join("All").join("Paper").join("metadata.toml"),
            "id = \"lr:test:edit\"\ntype = \"document\"\ntitle = \"Paper\"\n",
        )
        .unwrap();
        daemon.scan_all().unwrap();

        let item = daemon.get_item("lr:test:edit").unwrap().unwrap();
        assert!(
            item.categories.is_empty(),
            "the category must stay removed after a direct metadata edit",
        );
        assert!(
            !junction.exists(),
            "the orphaned junction must be removed to match metadata",
        );
    }

    #[test]
    fn category_in_metadata_without_a_junction_is_projected_on_scan() {
        // The reported bug: a category present in metadata.toml but with no Cat/
        // junction must NOT be shown as uncategorized. A scan projects the
        // junction and the item keeps its category.
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let item_dir = write_item(temp.path(), "lr:test:meta", "Paper");
        std::fs::write(
            item_dir.join("metadata.toml"),
            "id = \"lr:test:meta\"\ntype = \"document\"\ntitle = \"Paper\"\n\
             categories = [\"Wireless/RIS\"]\n",
        )
        .unwrap();

        daemon.scan_all().unwrap();

        let item = daemon.get_item("lr:test:meta").unwrap().unwrap();
        assert_eq!(
            item.categories,
            vec!["Wireless/RIS"],
            "a category in metadata must survive a scan, not become uncategorized",
        );
        assert!(
            temp.path().join("Cat").join("Wireless").join("RIS").join("Paper").exists(),
            "the junction must be projected from metadata",
        );
    }

    #[test]
    fn relocating_the_library_reprojects_junctions_instead_of_wiping_categories()
    {
        // NTFS Cat/ junctions do not survive a copy/restore/sync, but
        // metadata.toml and cat-manifest.toml do. A scan after a relocation
        // must NOT read the missing junctions as deliberate user deletions and
        // strip+tombstone every membership — it must re-project from metadata.
        let source = tempfile::tempdir().unwrap();
        {
            let daemon = LocalrefDaemon::for_library(source.path()).unwrap();
            write_item(source.path(), "lr:test:move", "Paper");
            daemon.scan_all().unwrap();
            daemon
                .add_item_category(
                    "lr:test:move",
                    CategoryPath::new("Inbox").unwrap(),
                )
                .unwrap();
        }

        // Simulate a relocation: copy All/ + .localref/ (which carry metadata
        // and the manifest) to a new root, but NOT the Cat/ junctions, which a
        // real copy across machines/drives cannot reproduce.
        let dest = tempfile::tempdir().unwrap();
        copy_dir_recursive(
            &source.path().join("All"),
            &dest.path().join("All"),
        );
        copy_dir_recursive(
            &source.path().join(".localref"),
            &dest.path().join(".localref"),
        );

        let daemon = LocalrefDaemon::for_library(dest.path()).unwrap();
        daemon.scan_all().unwrap();

        let item = daemon.get_item("lr:test:move").unwrap().unwrap();
        assert_eq!(
            item.categories,
            vec!["Inbox"],
            "relocation must preserve category membership, not wipe it",
        );
        let metadata = std::fs::read_to_string(
            dest.path().join("All").join("Paper").join("metadata.toml"),
        )
        .unwrap();
        let parsed = Metadata::from_toml_str(&metadata).unwrap();
        assert!(
            parsed.state.removed_categories.is_empty(),
            "relocation must not write spurious tombstones",
        );
        // And the junction must be re-projected under the new root.
        assert!(
            dest.path().join("Cat").join("Inbox").join("Paper").exists(),
            "the junction should be re-created from metadata after the move",
        );
    }

    /// Recursively copy a directory tree, following/ recreating plain files and
    /// subdirectories (used to simulate a library relocation in tests).
    fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let src = entry.path();
            let dst = to.join(entry.file_name());
            if src.is_dir() {
                copy_dir_recursive(&src, &dst);
            } else {
                std::fs::copy(&src, &dst).unwrap();
            }
        }
    }

    #[test]
    fn tombstoned_category_is_not_refiled_by_a_matching_rule() {
        // (b) Once a category is tombstoned, an auto-classification rule that
        // would otherwise match must not re-file the item into it.
        let temp = tempfile::tempdir().unwrap();
        let item_dir = write_item(temp.path(), "lr:test:tomb", "Paper");
        // Metadata already carries the tombstone for "Wireless/RIS".
        std::fs::write(
            item_dir.join("metadata.toml"),
            r#"id = "lr:test:tomb"
type = "journalArticle"
title = "RIS Paper"

[state]
removed_categories = ["Wireless/RIS"]
"#,
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join(".localref")).unwrap();
        std::fs::write(
            temp.path().join(".localref").join("rules.toml"),
            r#"
[[rules]]
name = "ris"
target = "Wireless/RIS"
query = 'title:RIS'
"#,
        )
        .unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon.scan_all().unwrap();

        // The scan reconciliation must respect the tombstone: no junction, no
        // metadata membership for the tombstoned category.
        let item = daemon.get_item("lr:test:tomb").unwrap().unwrap();
        assert!(
            !item.categories.contains(&"Wireless/RIS".to_string()),
            "a tombstoned category must not be re-filed",
        );
    }

    #[test]
    fn hand_made_junction_is_adopted_into_metadata_on_scan() {
        // (c) A junction the user creates by hand (present on disk, absent from
        // metadata and the manifest) is adopted into metadata on scan.
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let item_dir = write_item(temp.path(), "lr:test:adopt", "Paper");
        daemon.scan_all().unwrap();

        // Hand-make a junction under Cat/ with no metadata/manifest record.
        crate::platformfs::LibraryFs::new(temp.path())
            .create_category_link(
                &CategoryPath::new("Handmade").unwrap(),
                &item_dir,
            )
            .unwrap();
        daemon.scan_all().unwrap();

        let item = daemon.get_item("lr:test:adopt").unwrap().unwrap();
        assert_eq!(
            item.categories,
            vec!["Handmade"],
            "a hand-made junction must be adopted into metadata",
        );
    }

    // Verification item (d) — "a plain index rebuild reflects metadata
    // categories with no junction present" — is covered directly at the storage
    // layer by `storage::tests::rebuild_reads_categories_from_metadata`.

    #[test]
    fn set_item_extra_writes_metadata_and_survives_reindex() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        write_item(temp.path(), "lr:test:extra", "Paper");
        daemon.rebuild_index().unwrap();

        daemon
            .set_item_extra("lr:test:extra", "bibtexer", "cite_key", Some("smith2020"))
            .unwrap();

        // Written into metadata.toml (source of truth) ...
        let metadata = std::fs::read_to_string(
            temp.path().join("All").join("Paper").join("metadata.toml"),
        )
        .unwrap();
        assert!(metadata.contains("cite_key"));
        assert!(metadata.contains("smith2020"));
        // ... and surfaced on the indexed document.
        let item = daemon.get_item("lr:test:extra").unwrap().unwrap();
        assert_eq!(item.extra["bibtexer"]["cite_key"], "smith2020");
    }

    #[test]
    fn only_declared_indexed_extra_fields_are_searchable() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        write_item(temp.path(), "lr:test:idx", "Paper");
        // Declare bibtexer.cite_key indexed; rating.note is not declared.
        daemon
            .set_indexed_extra_fields(
                ["bibtexer.cite_key".to_string()].into_iter().collect(),
            )
            .unwrap();
        daemon
            .set_item_extra("lr:test:idx", "bibtexer", "cite_key", Some("zoodle77"))
            .unwrap();
        daemon
            .set_item_extra("lr:test:idx", "rating", "note", Some("wibblish"))
            .unwrap();

        // (b) The declared-indexed value is found; the undeclared one is not.
        assert_eq!(daemon.search("zoodle77").unwrap().len(), 1);
        assert!(daemon.search("wibblish").unwrap().is_empty());
    }

    #[test]
    fn rebuild_repopulates_extra_index_from_metadata() {
        // (c) The extra index is rebuildable purely from metadata.toml — set a
        // value, wipe/rebuild, and it is still searchable.
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        write_item(temp.path(), "lr:test:reidx", "Paper");
        daemon
            .set_indexed_extra_fields(
                ["bibtexer.cite_key".to_string()].into_iter().collect(),
            )
            .unwrap();
        daemon
            .set_item_extra("lr:test:reidx", "bibtexer", "cite_key", Some("froon42"))
            .unwrap();

        daemon.rebuild_index().unwrap();

        assert_eq!(daemon.search("froon42").unwrap().len(), 1);
    }
}
