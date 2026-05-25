//! User-facing REST API for Localref.
//!
//! This crate exposes query-oriented API endpoints over the rebuildable storage
//! database. It does not write `All/` or `Cat/` directly; scan requests rebuild
//! the query cache from filesystem truth through `storage`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use localref_core::{LocalrefDaemon, PauseMode, PendingImportConfirmation};
use localref_core::model::Metadata;
use serde::{Deserialize, Serialize};
use serde_json::json;
use localref_core::storage::StorageDb;
use localref_core::types::CategoryPath;

/// Shared API application state.
#[derive(Clone)]
pub struct ApiState {
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

/// Request body for importing an existing `All/<dir>` directory.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ImportFolderRequest {
    /// Absolute path or library-relative path to the directory.
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
        .route("/api/import/pending", get(pending_imports))
        .route(
            "/api/import/pending/{id}/confirm",
            post(confirm_pending_import),
        )
        .route("/api/import/pending/{id}/cancel", post(cancel_pending_import))
        .route("/api/items", get(list_items))
        .route("/api/items/{id}", get(get_item))
        .route(
            "/api/items/{id}/metadata",
            get(get_metadata).patch(patch_metadata),
        )
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
        .with_state(ApiState { daemon })
}

/// Open storage at `library_root` and build the API router.
pub fn router_for_library(
    library_root: impl Into<PathBuf>,
) -> localref_core::error::Result<Router> {
    Ok(router(StorageDb::open(library_root)?))
}

/// Run the user-facing API server until the process is stopped.
pub async fn serve(
    addr: SocketAddr,
    storage: StorageDb,
) -> std::io::Result<()> {
    serve_with_daemon(addr, LocalrefDaemon::new(storage)).await
}

/// Run the user-facing API server with an existing daemon facade.
pub async fn serve_with_daemon(
    addr: SocketAddr,
    daemon: LocalrefDaemon,
) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router_with_daemon(daemon)).await
}

async fn health() -> Response {
    Json(json!({
        "status": "ok",
        "service": "localref-rest"
    }))
    .into_response()
}

async fn scan(State(state): State<ApiState>) -> Response {
    match state.daemon.scan_all() {
        Ok(task) => Json(task).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn daemon_status(State(state): State<ApiState>) -> Response {
    Json(state.daemon.status()).into_response()
}

async fn pause(
    State(state): State<ApiState>,
    Json(request): Json<PauseRequest>,
) -> Response {
    Json(state.daemon.pause(request.mode)).into_response()
}

async fn resume(
    State(state): State<ApiState>,
    Json(request): Json<PauseRequest>,
) -> Response {
    Json(state.daemon.resume(request.mode)).into_response()
}

async fn list_items(State(state): State<ApiState>) -> Response {
    match state.daemon.list_items() {
        Ok(items) => Json(items).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn get_item(
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

/// Return the full metadata document and source revision for one item.
async fn get_metadata(
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

async fn patch_metadata(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<PatchMetadataRequest>,
) -> Response {
    match state.daemon.patch_metadata(
        &id,
        &request.expected_revision,
        request.metadata,
    ) {
        Ok(item) => Json(item).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn import_folder(
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

async fn import_file(
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

async fn normalize_cat_folder(
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

async fn add_item_category(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(request): Json<CategoryRequest>,
) -> Response {
    match state.daemon.add_item_category(&id, request.category) {
        Ok(summary) => Json(summary).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn create_category(
    State(state): State<ApiState>,
    Json(request): Json<CategoryRequest>,
) -> Response {
    match state.daemon.create_category(request.category) {
        Ok(summary) => Json(summary).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn remove_item_category(
    State(state): State<ApiState>,
    Path((id, category)): Path<(String, String)>,
) -> Response {
    let Some(category) = CategoryPath::new(category) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid category path");
    };
    match state.daemon.remove_item_category(&id, category) {
        Ok(summary) => Json(summary).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn rename_category(
    State(state): State<ApiState>,
    Json(request): Json<CategoryMoveRequest>,
) -> Response {
    match state.daemon.rename_category(request.from, request.to) {
        Ok(summary) => Json(summary).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn merge_category(
    State(state): State<ApiState>,
    Json(request): Json<CategoryMoveRequest>,
) -> Response {
    match state.daemon.merge_category(request.from, request.to) {
        Ok(summary) => Json(summary).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn search(
    State(state): State<ApiState>,
    Query(query): Query<HashMap<String, String>>,
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

async fn events(State(state): State<ApiState>) -> Response {
    match state.daemon.events() {
        Ok(events) => Json(events).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn events_stream(State(state): State<ApiState>) -> Response {
    match state.daemon.events() {
        Ok(events) => {
            let body = events
                .into_iter()
                .map(|event| {
                    let event_name = serde_json::to_value(&event.kind)
                        .expect("event kind should serialize")
                        .as_str()
                        .expect("event kind should serialize as a string")
                        .to_string();
                    let json = serde_json::to_string(&event)
                        .expect("event should serialize");
                    format!("event: {event_name}\ndata: {json}\n\n")
                })
                .collect::<String>();
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

async fn categories_tree(State(state): State<ApiState>) -> Response {
    match state.daemon.list_categories() {
        Ok(categories) => Json(categories).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn pending_imports(State(state): State<ApiState>) -> Response {
    Json(state.daemon.pending_imports()).into_response()
}

async fn confirm_pending_import(
    State(state): State<ApiState>,
    Path(id): Path<u64>,
    Json(request): Json<PendingImportConfirmation>,
) -> Response {
    match state.daemon.confirm_pending_import(id, request) {
        Ok(outcome) => Json(outcome).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

async fn cancel_pending_import(
    State(state): State<ApiState>,
    Path(id): Path<u64>,
) -> Response {
    match state.daemon.cancel_pending_import(id) {
        Ok(session) => Json(session).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

fn api_error(status: StatusCode, message: impl Into<String>) -> Response {
    let mut response =
        Json(json!({ "error": message.into() })).into_response();
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::body::to_bytes;
    use http::Request;
    use serde_json::Value;
    use tower::ServiceExt;
    use localref_core::types::{ConnectorImport, ConnectorItem};

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

        let events = request_json(&app, "GET", "/api/events").await;
        assert_eq!(events[0]["kind"], "scan_started");
        assert_eq!(events[1]["kind"], "scan_finished");
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
    async fn confirms_pending_imports() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let session = daemon
            .create_pending_connector_import(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-rest-pending".to_string()),
                    uri: None,
                    connector_item_id: Some("rest-pending".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "REST Pending Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "REST Pending Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();
        let app = router_with_daemon(daemon);

        let pending = request_json(&app, "GET", "/api/import/pending").await;
        assert_eq!(pending[0]["title"], "REST Pending Paper");

        let outcome = request_json_body(
            &app,
            "POST",
            &format!("/api/import/pending/{}/confirm", session.id),
            json!({"categories": ["Inbox"]}),
        )
        .await;
        assert_eq!(outcome["categories"][0], "Inbox");

        let remaining = request_json(&app, "GET", "/api/import/pending").await;
        assert_eq!(remaining.as_array().unwrap().len(), 0);
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
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("event: metadata_created"));
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

    async fn request_json(app: &Router, method: &str, uri: &str) -> Value {
        request_json_body(app, method, uri, Value::Null).await
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
}
