//! Zotero Connector-compatible HTTP surface for Localref.
//!
//! This crate is a protocol adapter for the browser extension named Zotero
//! Connector. It listens on the same loopback HTTP surface as Zotero Desktop,
//! accepts connector-shaped requests, and forwards bibliographic saves to an
//! injected [`ConnectorImportSink`]. Production code should wire the sink to
//! `core::ImportPipeline`; tests and the manual dynamic-test binary can use an
//! in-memory or logging sink.
//!
//! The adapter intentionally does not implement Zotero translators, Zotero's
//! database, collection semantics, sync, PDF recognition, or attachment
//! resolver logic. Those systems are either browser-side responsibilities or
//! later Localref pipeline work. This crate only keeps the connector request
//! sequence alive long enough to capture metadata and attachment uploads.
//!
//! # References
//!
//! - Zotero's public connector HTTP server documentation:
//!   <https://www.zotero.org/support/dev/client_coding/connector_http_server>
//! - Zotero Connector's browser-side RPC client, especially
//!   `Zotero.Connector.callMethod()`:
//!   <https://github.com/zotero/zotero-connectors/blob/master/src/common/connector.js>
//! - Zotero Connector's browser-side item save flow, especially
//!   `ItemSaver._saveToZotero()`:
//!   <https://github.com/zotero/zotero-connectors/blob/master/src/common/itemSaver.js>
//! - Zotero Desktop's connector server implementation:
//!   <https://github.com/zotero/zotero/blob/ba3c3a506fb8f3a1f7facc726c32624df9aa7981/chrome/content/zotero/xpcom/server/server_connector.js>
//!
//! # Source behavior this crate mirrors
//!
//! Zotero Connector builds requests as `connector/<method>` calls against the
//! configured connector URL. It sends JSON requests with `X-Zotero-Version` and
//! `X-Zotero-Connector-API-Version` headers, and treats any 4xx/5xx response as
//! an RPC failure. Zotero Desktop exposes methods such as `/connector/ping`,
//! `/connector/saveItems`, `/connector/saveSnapshot`,
//! `/connector/getTranslatorCode`, and `/connector/selectItems`.
//!
//! Zotero Desktop's `GET /connector/ping` returns a small `text/html` liveness
//! body. `POST /connector/ping` returns JSON preferences that influence the
//! browser-side save path. `POST /connector/saveItems` creates a save session
//! and normally returns `201 Created`. If `supportsAttachmentUpload` is true,
//! the browser extension uploads PDF/EPUB attachments with
//! `/connector/saveAttachment` after the top-level item save.
//!
//! # Localref implementation behavior
//!
//! Localref returns Zotero-compatible liveness, preference, status, and creation
//! responses, but records imports and upload events instead of saving into a
//! Zotero database. The normalized [`ConnectorImportRequest`] is the only stable
//! output of `/connector/saveItems`; attachment and snapshot calls are reported
//! as [`ConnectorEvent`] values so the future `core::ImportPipeline` can decide
//! how to stage and persist them.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, CONTENT_TYPE,
};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub mod rest;

use localref_core::types::{ConnectorAttachment, ConnectorItem};

/// The connector API version advertised by current Zotero Connectors.
///
/// Zotero source behavior: the extension sends this value in the
/// `X-Zotero-Connector-API-Version` request header, and Zotero Desktop compares
/// it with the server-side connector API version before accepting some calls.
///
/// Localref behavior: Localref echoes the same compatible value in responses so
/// the browser extension treats the dev server as an online connector target.
pub const CONNECTOR_API_VERSION: &str = "3";

/// Localref's advertised Zotero-compatible server version.
///
/// Zotero source behavior: Zotero Desktop responds with `X-Zotero-Version`,
/// which the browser extension stores as the connected client version after
/// successful RPC calls.
///
/// Localref behavior: this value is deliberately branded as Localref. It is a
/// compatibility marker, not an attempt to impersonate a real Zotero release.
pub const LOCALREF_CONNECTOR_VERSION: &str = "Localref-0.1.0";

/// Default Zotero Connector HTTP server address.
///
/// Zotero source behavior: Zotero Desktop normally listens on
/// `127.0.0.1:23119`, and the browser extension's connector URL preference
/// points to that origin.
///
/// Localref behavior: the manual dynamic-test binary uses the same address so
/// an unmodified Zotero Connector can discover Localref when Zotero Desktop is
/// not already occupying the port.
pub const DEFAULT_CONNECTOR_ADDR: &str =
    localref_core::config::DEFAULT_CSC_ADDR;

/// A request accepted from Zotero Connector's `saveItems` method.
///
/// Zotero source behavior: the browser-side item saver sends `sessionID`,
/// `uri`, optional `proxy`, optional `singleFile`, and `items` after translators
/// have produced Zotero JSON item arrays.
///
/// Localref behavior: this struct preserves the connector payload fields needed
/// to create a [`ConnectorImportRequest`]. The full item objects remain
/// [`Value`] because the `model` crate should own the eventual metadata mapping.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveItemsRequest {
    /// Zotero save session identifier.
    #[serde(default, alias = "sessionID", alias = "sessionId")]
    pub session_id: Option<String>,
    /// Page URI that produced the translated items.
    #[serde(default)]
    pub uri: Option<String>,
    /// Optional Zotero proxy description.
    #[serde(default)]
    pub proxy: Option<Value>,
    /// Translated items in Zotero item-array format.
    #[serde(default)]
    pub items: Vec<Value>,
    /// Whether the connector expects old single-file snapshot handling.
    #[serde(default)]
    pub single_file: Option<bool>,
}

/// Import request emitted by this adapter.
///
/// Zotero source behavior: Zotero Desktop turns the connector item JSON into
/// Zotero DB objects inside a save session.
///
/// Localref behavior: Localref only normalizes the session id, source URI, and
/// raw item payloads. The injected sink is responsible for forwarding this to
/// the future `core::ImportPipeline`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConnectorImportRequest {
    /// Zotero save session identifier.
    pub session_id: Option<String>,
    /// Page URI that produced the translated items.
    pub uri: Option<String>,
    /// Translated item payloads as received from Zotero Connector.
    pub items: Vec<Value>,
    /// Translated item payloads normalized into shared Localref types.
    pub normalized_items: Vec<ConnectorItem>,
}

/// Connector request event useful for diagnostics and later attachment handling.
///
/// Zotero source behavior: Zotero Desktop stores progress and attachment state
/// in a save session, then lets follow-up endpoints mutate that session.
///
/// Localref behavior: session mutation is not implemented yet, so follow-up
/// calls are emitted as events. The manual dynamic-test binary prints these
/// events to prove that the browser extension is reaching Localref.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectorEvent {
    /// A connector method was called.
    MethodCalled {
        /// HTTP method used by the connector.
        method: String,
        /// Connector endpoint path.
        path: String,
    },
    /// A binary attachment upload was received from the connector.
    AttachmentReceived {
        /// Save session identifier.
        session_id: Option<String>,
        /// Attachment metadata supplied in the `X-Metadata` header.
        metadata: Option<Value>,
        /// Uploaded byte count.
        bytes: usize,
    },
    /// A webpage snapshot request was received.
    SnapshotReceived {
        /// Save session identifier.
        session_id: Option<String>,
        /// Page URI for the snapshot.
        uri: Option<String>,
        /// Snapshot title, if supplied.
        title: Option<String>,
    },
}

/// Receives normalized connector imports and diagnostic events.
///
/// Zotero source behavior: Zotero Desktop's endpoint handlers call directly
/// into Zotero save-session and item-save code.
///
/// Localref behavior: endpoints depend only on this trait. This keeps `csc`
/// independent from `core` while still providing a single integration point for
/// the future `core::ImportPipeline`.
pub trait ConnectorImportSink: Send + Sync + 'static {
    /// Accept one normalized connector import request.
    fn accept_import(
        &self,
        request: ConnectorImportRequest,
    ) -> Result<(), String>;

    /// Accept one uploaded connector attachment.
    fn accept_attachment(
        &self,
        attachment: ConnectorAttachment,
    ) -> Result<(), String>;

    /// Accept one connector event.
    fn accept_event(&self, event: ConnectorEvent) -> Result<(), String>;
}

/// In-memory sink used by tests.
///
/// Zotero source behavior: Zotero Desktop uses persistent save sessions.
///
/// Localref behavior: tests only need deterministic observation of accepted
/// imports and events, so this sink stores them in process memory.
#[derive(Debug, Default)]
pub struct MemoryImportSink {
    imports: Mutex<Vec<ConnectorImportRequest>>,
    attachments: Mutex<Vec<ConnectorAttachment>>,
    events: Mutex<Vec<ConnectorEvent>>,
}

impl MemoryImportSink {
    /// Return all imports accepted by this sink.
    pub fn imports(&self) -> Vec<ConnectorImportRequest> {
        self.imports.lock().expect("memory import sink mutex poisoned").clone()
    }

    /// Return all connector events accepted by this sink.
    pub fn events(&self) -> Vec<ConnectorEvent> {
        self.events
            .lock()
            .expect("memory connector event sink mutex poisoned")
            .clone()
    }

    /// Return all attachments accepted by this sink.
    pub fn attachments(&self) -> Vec<ConnectorAttachment> {
        self.attachments
            .lock()
            .expect("memory connector attachment sink mutex poisoned")
            .clone()
    }
}

impl ConnectorImportSink for MemoryImportSink {
    fn accept_import(
        &self,
        request: ConnectorImportRequest,
    ) -> Result<(), String> {
        self.imports
            .lock()
            .expect("memory import sink mutex poisoned")
            .push(request);
        Ok(())
    }

    fn accept_attachment(
        &self,
        attachment: ConnectorAttachment,
    ) -> Result<(), String> {
        self.attachments
            .lock()
            .expect("memory connector attachment sink mutex poisoned")
            .push(attachment);
        Ok(())
    }

    fn accept_event(&self, event: ConnectorEvent) -> Result<(), String> {
        self.events
            .lock()
            .expect("memory connector event sink mutex poisoned")
            .push(event);
        Ok(())
    }
}

#[derive(Clone)]
struct AppState {
    sink: Arc<dyn ConnectorImportSink>,
}

/// Build the Zotero Connector-compatible router.
///
/// Zotero source behavior: Zotero Desktop registers endpoint objects under
/// paths such as `/connector/ping`, `/connector/saveItems`,
/// `/connector/saveSnapshot`, and `/connector/saveAttachment`.
///
/// Localref behavior: this function registers the subset needed for browser
/// Connector liveness, item save, snapshot, collection, progress, and
/// attachment-upload flows. Unsupported endpoints fall through to a JSON 404
/// response so the manual test server surfaces unexpected connector calls.
pub fn router(sink: Arc<dyn ConnectorImportSink>) -> Router {
    let state = AppState { sink };

    Router::new()
        .route(
            "/connector/ping",
            get(ping_get).post(ping_post).options(preflight),
        )
        .route(
            "/connector/getTranslators",
            post(get_translators).options(preflight),
        )
        .route("/connector/detect", post(detect).options(preflight))
        .route(
            "/connector/getTranslatorCode",
            post(get_translator_code).options(preflight),
        )
        .route("/connector/saveItems", post(save_items).options(preflight))
        .route(
            "/connector/saveSnapshot",
            post(save_snapshot).options(preflight),
        )
        .route("/connector/selectItems", post(select_items).options(preflight))
        .route(
            "/connector/getSelectedCollection",
            get(selected_collection)
                .post(selected_collection)
                .options(preflight),
        )
        .route(
            "/connector/sessionProgress",
            post(session_progress).options(preflight),
        )
        .route(
            "/connector/attachmentProgress",
            post(attachment_progress).options(preflight),
        )
        .route(
            "/connector/hasAttachmentResolvers",
            post(has_attachment_resolvers).options(preflight),
        )
        .route(
            "/connector/saveAttachmentFromResolver",
            post(save_attachment_from_resolver).options(preflight),
        )
        .route(
            "/connector/saveAttachment",
            post(save_attachment).options(preflight),
        )
        .route(
            "/connector/saveStandaloneAttachment",
            post(save_attachment).options(preflight),
        )
        .route(
            "/connector/saveSingleFile",
            post(save_single_file).options(preflight),
        )
        .route(
            "/connector/updateSession",
            post(update_session).options(preflight),
        )
        .route("/connector/delaySync", post(delay_sync).options(preflight))
        .route(
            "/connector/getRecognizedItem",
            post(get_recognized_item).options(preflight),
        )
        .fallback(connector_not_found)
        .with_state(state)
}

/// Run a connector-compatible HTTP server until the process is stopped.
///
/// Zotero source behavior: Zotero Desktop owns a long-lived loopback HTTP
/// server on the connector port.
///
/// Localref behavior: this helper binds a `tokio` TCP listener and serves the
/// router built by [`router`]. It is used by the manual dynamic-test binary and
/// can later be embedded by the Localref daemon.
pub async fn serve(
    addr: SocketAddr,
    sink: Arc<dyn ConnectorImportSink>,
) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(sink)).await
}

/// Handle `GET /connector/ping`.
///
/// Zotero source behavior: the desktop server returns `text/html` containing a
/// short liveness string. This is the quickest way to prove the connector port
/// is occupied by a compatible server.
///
/// Localref behavior: Localref returns the same liveness body and records a
/// method-call event for manual debugging.
async fn ping_get(State(state): State<AppState>) -> Response {
    record_method(&state, "GET", "/connector/ping");
    text_response(StatusCode::OK, "text/html", "Zotero is running")
}

/// Handle `POST /connector/ping`.
///
/// Zotero source behavior: Zotero Desktop returns preferences such as
/// `downloadAssociatedFiles`, `automaticSnapshots`, and
/// `supportsAttachmentUpload`; the browser extension uses these values to
/// choose whether to upload attachments directly.
///
/// Localref behavior: the response enables direct attachment upload and
/// disables features Localref does not yet support, such as tag autocomplete and
/// Google Docs integration.
async fn ping_post(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/ping");
    json_response(json!({
        "prefs": {
            "automaticSnapshots": false,
            "downloadAssociatedFiles": true,
            "supportsAttachmentUpload": true,
            "supportsTagsAutocomplete": false,
            "googleDocsAddNoteEnabled": false,
            "googleDocsCitationExplorerEnabled": false,
            "canUserAddNote": false,
            "reportActiveURL": true
        }
    }))
}

/// Handle `POST /connector/getTranslators`.
///
/// Zotero source behavior: Zotero Desktop returns translator metadata,
/// including translator records that the browser extension can cache.
///
/// Localref behavior: Localref does not serve translators. Returning an empty
/// list lets this endpoint succeed without claiming translator support.
async fn get_translators(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/getTranslators");
    json_response(Vec::<Value>::new())
}

/// Handle `POST /connector/detect`.
///
/// Zotero source behavior: Zotero Desktop can detect translators by loading
/// supplied HTML into a hidden document and running translator detection.
///
/// Localref behavior: translator detection is not implemented, so this endpoint
/// returns an empty list. Normal saves can still work when the extension has
/// already translated items itself.
async fn detect(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/detect");
    json_response(Vec::<Value>::new())
}

/// Handle `POST /connector/getTranslatorCode`.
///
/// Zotero source behavior: Zotero Desktop returns JavaScript source for a
/// requested translator id.
///
/// Localref behavior: translator code is not bundled in Localref, so this
/// endpoint returns 404 instead of fabricating translator source.
async fn get_translator_code(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/getTranslatorCode");
    text_response(StatusCode::NOT_FOUND, "text/plain", "translator not found")
}

/// Handle `POST /connector/saveItems`.
///
/// Zotero source behavior: Zotero Desktop creates a save session, persists
/// top-level bibliographic items, and returns `201 Created` on success.
///
/// Localref behavior: Localref records the normalized item payload through
/// [`ConnectorImportSink::accept_import`] and returns `201 Created`. Metadata
/// mapping, staging, rule classification, and category confirmation belong to
/// the future `core::ImportPipeline`.
async fn save_items(
    State(state): State<AppState>,
    Json(payload): Json<SaveItemsRequest>,
) -> Response {
    record_method(&state, "POST", "/connector/saveItems");
    let normalized_items = match payload
        .items
        .iter()
        .map(|item| connector_item_from_value(&payload, item))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(items) => items,
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, message);
        }
    };
    let import = ConnectorImportRequest {
        session_id: payload.session_id.clone(),
        uri: payload.uri.clone(),
        items: payload.items.clone(),
        normalized_items,
    };

    match state.sink.accept_import(import) {
        Ok(()) => empty_json_response(StatusCode::CREATED),
        Err(message) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
        }
    }
}

/// Handle `POST /connector/saveSnapshot`.
///
/// Zotero source behavior: Zotero Desktop creates a top-level webpage item and
/// may store a snapshot attachment for pages without a specific translator.
///
/// Localref behavior: Localref records a [`ConnectorEvent::SnapshotReceived`]
/// and returns `201 Created`, leaving snapshot persistence to the later import
/// pipeline.
async fn save_snapshot(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Response {
    record_method(&state, "POST", "/connector/saveSnapshot");
    let _ = state.sink.accept_event(ConnectorEvent::SnapshotReceived {
        session_id: json_string(&payload, &["sessionID", "sessionId"]),
        uri: json_string(&payload, &["uri", "url"]),
        title: json_string(&payload, &["title"]),
    });
    empty_json_response(StatusCode::CREATED)
}

/// Handle `POST /connector/selectItems`.
///
/// Zotero source behavior: Zotero Desktop displays a selection dialog for
/// multi-item translator results and returns the selected item ids.
///
/// Localref behavior: there is no UI bridge here yet, so the endpoint selects
/// every supplied key. This keeps the connector flow non-interactive until the
/// planned Dioxus import-confirmation UI is wired through `rest`.
async fn select_items(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Response {
    record_method(&state, "POST", "/connector/selectItems");
    let mut selected = serde_json::Map::new();
    if let Some(items) = payload.get("items").and_then(Value::as_object) {
        for key in items.keys() {
            selected.insert(key.clone(), Value::Bool(true));
        }
    }
    json_response(Value::Object(selected))
}

/// Handle `/connector/getSelectedCollection`.
///
/// Zotero source behavior: Zotero Desktop returns the active library or
/// collection, editable flags, possible save targets, and tags.
///
/// Localref behavior: Localref advertises a single editable "Localref" target.
/// Future category selection belongs to the Localref UI, not Zotero collection
/// selection.
async fn selected_collection(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/getSelectedCollection");
    json_response(json!({
        "libraryID": 1,
        "libraryName": "Localref",
        "libraryEditable": true,
        "id": null,
        "name": "Localref",
        "editable": true,
        "filesEditable": true,
        "targets": [{
            "id": "L1",
            "name": "Localref",
            "filesEditable": true,
            "level": 0
        }],
        "tags": {}
    }))
}

/// Handle `POST /connector/sessionProgress`.
///
/// Zotero source behavior: Zotero Desktop returns save-session progress for
/// attachment polling.
///
/// Localref behavior: this adapter reports the session as immediately done.
/// Attachments are accepted by upload endpoints and emitted as events.
async fn session_progress(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/sessionProgress");
    json_response(json!({
        "done": true,
        "items": []
    }))
}

/// Handle `POST /connector/attachmentProgress`.
///
/// Zotero source behavior: some connector flows poll attachment progress
/// separately from session progress.
///
/// Localref behavior: this endpoint reports no pending attachments and marks
/// the progress query complete.
async fn attachment_progress(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/attachmentProgress");
    json_response(json!({
        "done": true,
        "attachments": []
    }))
}

/// Handle `POST /connector/hasAttachmentResolvers`.
///
/// Zotero source behavior: Zotero Desktop checks whether open-access or custom
/// resolvers can find a missing primary attachment.
///
/// Localref behavior: resolver support is out of scope for this adapter, so it
/// always returns `false`.
async fn has_attachment_resolvers(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/hasAttachmentResolvers");
    json_response(false)
}

/// Handle `POST /connector/saveAttachmentFromResolver`.
///
/// Zotero source behavior: Zotero Desktop may download an attachment from an
/// open-access resolver and attach it to an item in the save session.
///
/// Localref behavior: because [`has_attachment_resolvers`] always returns
/// `false`, this endpoint should normally not be called. It returns 404 if it
/// is called.
async fn save_attachment_from_resolver(
    State(state): State<AppState>,
) -> Response {
    record_method(&state, "POST", "/connector/saveAttachmentFromResolver");
    error_response(
        StatusCode::NOT_FOUND,
        "Localref has no attachment resolver in the connector adapter",
    )
}

/// Handle `POST /connector/saveAttachment` and `saveStandaloneAttachment`.
///
/// Zotero source behavior: when `supportsAttachmentUpload` is true, Zotero
/// Connector uploads PDF/EPUB bytes with attachment metadata in the
/// `X-Metadata` header.
///
/// Localref behavior: this adapter records the metadata and byte count as a
/// [`ConnectorEvent::AttachmentReceived`], forwards the uploaded bytes through
/// [`ConnectorImportSink::accept_attachment`], and returns `201 Created`.
async fn save_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    record_method(&state, "POST", "/connector/saveAttachment");
    let metadata = parse_attachment_metadata(&headers);
    let session_id = metadata
        .as_ref()
        .and_then(|value| json_string(value, &["sessionID", "sessionId"]));
    let _ = state.sink.accept_event(ConnectorEvent::AttachmentReceived {
        session_id: session_id.clone(),
        metadata: metadata.clone(),
        bytes: body.len(),
    });

    match connector_attachment_from_upload(session_id, metadata, body) {
        Ok(attachment) => match state.sink.accept_attachment(attachment) {
            Ok(()) => empty_json_response(StatusCode::CREATED),
            Err(message) => {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
            }
        },
        Err(message) => error_response(StatusCode::BAD_REQUEST, message),
    }
}

/// Handle `POST /connector/saveSingleFile`.
///
/// Zotero source behavior: Zotero Connector can send SingleFile snapshot
/// content for webpage snapshots.
///
/// Localref behavior: this adapter records the snapshot request and returns
/// `201 Created`; snapshot content persistence is not implemented here.
async fn save_single_file(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Response {
    record_method(&state, "POST", "/connector/saveSingleFile");
    let _ = state.sink.accept_event(ConnectorEvent::SnapshotReceived {
        session_id: json_string(&payload, &["sessionID", "sessionId"]),
        uri: json_string(&payload, &["url", "uri"]),
        title: json_string(&payload, &["title"]),
    });
    empty_json_response(StatusCode::CREATED)
}

/// Handle `POST /connector/updateSession`.
///
/// Zotero source behavior: Zotero Desktop updates a save session's target,
/// tags, or note after user interaction in the connector popup.
///
/// Localref behavior: session mutation is not implemented yet, so this endpoint
/// accepts the call and returns an empty JSON object.
async fn update_session(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/updateSession");
    json_response(json!({}))
}

/// Handle `POST /connector/delaySync`.
///
/// Zotero source behavior: Zotero Desktop can delay sync after connector
/// operations.
///
/// Localref behavior: Localref has no sync runner in this crate, so this is a
/// no-op `204 No Content`.
async fn delay_sync(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/delaySync");
    empty_response(StatusCode::NO_CONTENT)
}

/// Handle `POST /connector/getRecognizedItem`.
///
/// Zotero source behavior: Zotero Desktop can return metadata recognized from a
/// standalone PDF/EPUB attachment.
///
/// Localref behavior: recognition is out of scope here, so the endpoint returns
/// `204 No Content`.
async fn get_recognized_item(State(state): State<AppState>) -> Response {
    record_method(&state, "POST", "/connector/getRecognizedItem");
    empty_response(StatusCode::NO_CONTENT)
}

/// Handle CORS preflight requests for connector endpoints.
///
/// Zotero source behavior: browser extension requests are ordinary extension
/// HTTP requests, while web pages may also issue limited requests to the
/// loopback server. The desktop server therefore needs predictable method and
/// content-type handling.
///
/// Localref behavior: Localref permits the methods and headers used by Zotero
/// Connector, including the `X-Metadata` header used for attachment uploads.
async fn preflight() -> Response {
    with_connector_headers(StatusCode::OK.into_response())
}

/// Return a JSON error for connector methods not implemented by Localref.
///
/// Zotero source behavior: unsupported endpoints do not exist in the desktop
/// endpoint registry.
///
/// Localref behavior: a structured 404 makes the manual dynamic-test server
/// easier to diagnose when the browser extension calls an endpoint not yet
/// modeled in this crate.
async fn connector_not_found(request: Request<Body>) -> Response {
    if request.method() == Method::OPTIONS {
        return preflight().await;
    }

    error_response(
        StatusCode::NOT_FOUND,
        format!("unsupported connector endpoint: {}", request.uri().path()),
    )
}

/// Record a connector method call without failing the HTTP request.
///
/// Zotero source behavior: calls are normally visible through Zotero debug logs
/// and save-session state.
///
/// Localref behavior: method-call events are best-effort diagnostics. A logging
/// sink failure should not make the compatibility endpoint fail.
fn record_method(state: &AppState, method: &str, path: &str) {
    let _ = state.sink.accept_event(ConnectorEvent::MethodCalled {
        method: method.to_string(),
        path: path.to_string(),
    });
}

/// Extract the first string field matching any candidate key.
///
/// Zotero source behavior: connector payloads historically use `sessionID`,
/// while Rust and many JSON APIs would naturally prefer `sessionId`.
///
/// Localref behavior: both spellings are accepted wherever a session id is read
/// from untyped JSON.
fn json_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

/// Parse Zotero's `X-Metadata` attachment upload header.
fn parse_attachment_metadata(headers: &HeaderMap) -> Option<Value> {
    headers
        .get("X-Metadata")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
}

/// Convert an uploaded attachment body into the shared connector attachment type.
fn connector_attachment_from_upload(
    session_id: Option<String>,
    metadata: Option<Value>,
    body: Bytes,
) -> Result<ConnectorAttachment, String> {
    let mime_type = metadata
        .as_ref()
        .and_then(|value| json_string(value, &["mimeType", "contentType"]));
    let attachment_type = metadata.as_ref().and_then(|value| {
        json_string(value, &["type", "attachmentType", "kind"])
    });
    let filename = metadata
        .as_ref()
        .and_then(|value| {
            json_string(value, &["filename", "title", "path", "url"])
                .and_then(|value| filename_from_value(&value))
        })
        .map(|filename| {
            filename_with_type_extension(
                &filename,
                mime_type.as_deref(),
                attachment_type.as_deref(),
            )
        })
        .ok_or_else(|| {
            "attachment upload missing filename/title/path metadata"
                .to_string()
        })?;
    let parent_item_id = metadata.as_ref().and_then(|value| {
        json_string(value, &["parentItem", "parentItemID", "itemID"])
    });
    let title = metadata
        .as_ref()
        .and_then(|value| json_string(value, &["title", "filename", "name"]));

    Ok(ConnectorAttachment {
        session_id,
        parent_item_id,
        title,
        filename,
        mime_type,
        bytes: body.to_vec(),
        raw_metadata: metadata,
    })
}

/// Convert one translated Zotero item into a shared connector item.
fn connector_item_from_value(
    request: &SaveItemsRequest,
    item: &Value,
) -> Result<ConnectorItem, String> {
    let item_type = json_string(item, &["itemType", "type"]);
    let title = item_title(item, item_type.as_deref());
    Ok(ConnectorItem {
        session_id: request.session_id.clone(),
        uri: request
            .uri
            .clone()
            .or_else(|| json_string(item, &["url", "uri"])),
        connector_item_id: json_string(item, &["id", "itemID", "key"]),
        item_type,
        title,
        abstract_note: json_string(item, &["abstractNote", "abstract"]),
        doi: json_string(item, &["DOI", "doi"]),
        raw: item.clone(),
    })
}

/// Build a stable display title for any Zotero item type.
///
/// Reference source:
/// - <https://github.com/zotero/zotero-connectors/blob/master/src/common/itemSaver.js>
///   builds the `saveItems` payload from translated items, assigns an item id,
///   and sends the item objects to Zotero Desktop.
/// - <https://github.com/zotero/zotero-schema> defines the Zotero item type
///   vocabulary and the type-specific fields.
///
/// Zotero source behavior: translated items are not normalized to a single
/// title field before `saveItems`; types such as cases and emails can carry
/// their primary display text in fields like `caseName` or `subject`.
///
/// Localref behavior: connector import accepts every known Zotero item type,
/// keeps the full raw JSON on the normalized item, and uses this helper only to
/// choose a stable folder/display name for the `All/` import path.
///
/// Zotero's schema includes types whose primary user-facing field is not named
/// `title`, such as `caseName` for cases and `subject` for emails. Connector
/// imports should still create an `All/` directory instead of rejecting these
/// items, so this function uses known title-like fields and finally falls back
/// to the item type plus Zotero id.
fn item_title(item: &Value, item_type: Option<&str>) -> String {
    if let Some(title) = json_string(
        item,
        &[
            "title",
            "shortTitle",
            "caseName",
            "nameOfAct",
            "subject",
            "bookTitle",
            "publicationTitle",
            "proceedingsTitle",
            "encyclopediaTitle",
            "dictionaryTitle",
            "blogTitle",
            "websiteTitle",
            "forumTitle",
            "programTitle",
            "documentNumber",
            "reportNumber",
            "patentNumber",
            "billNumber",
            "docketNumber",
            "code",
        ],
    ) {
        return title;
    }

    if let Some(note) = json_string(item, &["note"]) {
        let stripped = strip_html_like_text(&note);
        if !stripped.is_empty() {
            return truncate_title(&stripped);
        }
    }

    if let Some(url) = json_string(item, &["url", "uri"]) {
        return url;
    }

    let item_type = item_type.unwrap_or("item");
    let id = json_string(item, &["id", "itemID", "key"])
        .unwrap_or_else(|| "untitled".to_string());
    format!("{item_type}-{id}")
}

/// Strip simple HTML/XML tags from a note body before it is used as a title.
///
/// Zotero Connector can send standalone notes whose useful user-facing text is
/// in a `note` field rather than in `title`. Localref only uses this stripped
/// text as an import folder/display fallback; the original note HTML remains in
/// the raw Zotero JSON.
fn strip_html_like_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Shorten long note-derived titles to a filesystem-friendly display length.
fn truncate_title(value: &str) -> String {
    const MAX_CHARS: usize = 80;
    let mut iter = value.chars();
    let title: String = iter.by_ref().take(MAX_CHARS).collect();
    if iter.next().is_some() { format!("{title}...") } else { title }
}

/// Extract a file name from connector attachment metadata.
fn filename_from_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|filename| !filename.trim().is_empty())
        .map(str::to_string)
}

/// Add a file extension inferred from attachment type when the name has none.
fn filename_with_type_extension(
    filename: &str,
    mime_type: Option<&str>,
    attachment_type: Option<&str>,
) -> String {
    if std::path::Path::new(filename).extension().is_some() {
        return filename.to_string();
    }

    match extension_for_attachment(mime_type, attachment_type) {
        Some(extension) => format!("{filename}.{extension}"),
        None => filename.to_string(),
    }
}

/// Infer a file extension from Zotero attachment MIME/type metadata.
fn extension_for_attachment(
    mime_type: Option<&str>,
    attachment_type: Option<&str>,
) -> Option<&'static str> {
    let mime = mime_type.map(str::to_ascii_lowercase);
    match mime.as_deref() {
        Some("application/pdf") => return Some("pdf"),
        Some("application/epub+zip") => return Some("epub"),
        Some("text/html") => return Some("html"),
        Some("text/plain") => return Some("txt"),
        Some("image/jpeg") => return Some("jpg"),
        Some("image/png") => return Some("png"),
        _ => {}
    }

    let attachment_type = attachment_type.map(str::to_ascii_lowercase);
    match attachment_type.as_deref() {
        Some("pdf") => Some("pdf"),
        Some("epub") => Some("epub"),
        Some("snapshot") | Some("webpage") | Some("html") => Some("html"),
        _ => None,
    }
}

/// Build a JSON connector response with Zotero-compatible headers.
///
/// Zotero source behavior: JSON endpoint handlers return an explicit
/// `application/json` MIME type and connector version headers.
///
/// Localref behavior: every JSON response passes through this helper so CORS and
/// connector headers stay consistent.
fn json_response<T>(value: T) -> Response
where
    T: Serialize,
{
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    with_connector_headers(response)
}

/// Build an empty JSON-typed connector response with a chosen status.
///
/// Zotero source behavior: `saveItems`, `saveSnapshot`, and attachment-save
/// endpoints often return `201 Created` without requiring a JSON body.
///
/// Localref behavior: this helper preserves the JSON content type and connector
/// headers even when the body is empty.
fn empty_json_response(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    with_connector_headers(response)
}

/// Build an empty connector response without a JSON content type.
///
/// Zotero source behavior: some endpoints return `204 No Content` when there is
/// no recognized item or no delayed-sync body.
///
/// Localref behavior: the helper is used for no-op compatibility endpoints.
fn empty_response(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    with_connector_headers(response)
}

/// Build a text connector response with Zotero-compatible headers.
///
/// Zotero source behavior: `GET /connector/ping` returns `text/html`, and some
/// resolver-related endpoints may return plain text.
///
/// Localref behavior: text responses are limited to liveness and
/// not-implemented compatibility paths.
fn text_response(
    status: StatusCode,
    content_type: &'static str,
    body: &'static str,
) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    with_connector_headers(response)
}

/// Build a structured JSON error response.
///
/// Zotero source behavior: connector failures commonly use JSON objects with an
/// `error` field for machine-readable failure handling.
///
/// Localref behavior: this helper is used for unsupported or failed operations
/// while preserving connector headers.
fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let mut response =
        Json(json!({ "error": message.into() })).into_response();
    *response.status_mut() = status;
    with_connector_headers(response)
}

/// Attach Zotero connector compatibility and CORS headers.
///
/// Zotero source behavior: the browser extension reads `X-Zotero-Version` and
/// uses connector API version headers to detect incompatible desktop clients.
///
/// Localref behavior: every response advertises the Localref compatibility
/// version and allows the headers/methods used by Zotero Connector RPC and
/// attachment upload flows.
fn with_connector_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    set_static_header(headers, "X-Zotero-Version", LOCALREF_CONNECTOR_VERSION);
    set_static_header(
        headers,
        "X-Zotero-Connector-API-Version",
        CONNECTOR_API_VERSION,
    );
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(
            "content-type,x-zotero-version,x-zotero-connector-api-version,x-metadata",
        ),
    );
    headers.insert(
        ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(
            "x-zotero-version,x-zotero-connector-api-version",
        ),
    );
    response
}

/// Insert a static header value into an HTTP header map.
///
/// Zotero source behavior: connector headers are fixed strings for a running
/// server process.
///
/// Localref behavior: using static header values keeps response construction
/// simple and avoids allocation for fixed compatibility headers.
fn set_static_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &'static str,
) {
    headers.insert(name, HeaderValue::from_static(value));
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use http::header::CONTENT_TYPE;
    use tower::ServiceExt;

    #[tokio::test]
    async fn get_ping_matches_zotero_server_shape() {
        let sink = Arc::new(MemoryImportSink::default());
        let response = router(sink)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/connector/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html");
        assert_eq!(to_text(response).await, "Zotero is running");
    }

    #[tokio::test]
    async fn post_ping_returns_connector_headers_and_preferences() {
        let sink = Arc::new(MemoryImportSink::default());
        let response = router(sink)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/connector/ping")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["X-Zotero-Connector-API-Version"],
            CONNECTOR_API_VERSION
        );

        let body = to_json(response).await;
        assert_eq!(body["prefs"]["supportsAttachmentUpload"], true);
    }

    #[tokio::test]
    async fn save_items_records_import_and_returns_created() {
        let sink = Arc::new(MemoryImportSink::default());
        let app = router(sink.clone());
        let payload = json!({
            "sessionID": "session-1",
            "uri": "https://example.test/paper",
            "items": [{
                "id": "abc123",
                "itemType": "journalArticle",
                "title": "A Localref Test Paper"
            }]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/connector/saveItems")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);

        let imports = sink.imports();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].session_id.as_deref(), Some("session-1"));
        assert_eq!(imports[0].items[0]["id"], "abc123");
        assert_eq!(
            imports[0].normalized_items[0].title,
            "A Localref Test Paper"
        );
    }

    #[test]
    fn connector_items_accept_all_known_zotero_item_types() {
        let request = SaveItemsRequest {
            session_id: Some("session-all-types".to_string()),
            uri: None,
            proxy: None,
            items: Vec::new(),
            single_file: None,
        };

        for item_type in known_zotero_item_types() {
            let item = json!({
                "id": item_type,
                "itemType": item_type
            });
            let normalized =
                connector_item_from_value(&request, &item).unwrap();

            assert_eq!(normalized.item_type.as_deref(), Some(*item_type));
            assert!(!normalized.title.trim().is_empty());
        }
    }

    #[test]
    fn connector_item_title_uses_type_specific_fields() {
        let request = SaveItemsRequest {
            session_id: Some("session-specific-title".to_string()),
            uri: None,
            proxy: None,
            items: Vec::new(),
            single_file: None,
        };

        let cases = [
            (
                json!({"itemType": "case", "caseName": "Smith v. Jones"}),
                "Smith v. Jones",
            ),
            (
                json!({"itemType": "email", "subject": "Archive request"}),
                "Archive request",
            ),
            (json!({"itemType": "bill", "billNumber": "HB 123"}), "HB 123"),
            (json!({"itemType": "patent", "patentNumber": "US123"}), "US123"),
            (
                json!({"itemType": "statute", "nameOfAct": "Example Act"}),
                "Example Act",
            ),
        ];

        for (item, expected_title) in cases {
            let normalized =
                connector_item_from_value(&request, &item).unwrap();
            assert_eq!(normalized.title, expected_title);
        }
    }

    #[test]
    fn connector_item_title_uses_note_text_when_title_is_absent() {
        let request = SaveItemsRequest {
            session_id: Some("session-note-title".to_string()),
            uri: None,
            proxy: None,
            items: Vec::new(),
            single_file: None,
        };
        let item = json!({
            "id": "note-1",
            "itemType": "note",
            "note": "<p>Important note <b>with HTML</b></p>"
        });

        let normalized = connector_item_from_value(&request, &item).unwrap();

        assert_eq!(normalized.title, "Important note with HTML");
        assert_eq!(
            normalized.raw["note"],
            "<p>Important note <b>with HTML</b></p>"
        );
    }

    #[tokio::test]
    async fn save_attachment_records_metadata_and_bytes() {
        let sink = Arc::new(MemoryImportSink::default());
        let response = router(sink.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/connector/saveAttachment")
                    .header(
                        "X-Metadata",
                        r#"{"sessionID":"s1","title":"paper.pdf"}"#,
                    )
                    .body(Body::from("pdf bytes"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert!(sink.events().iter().any(|event| matches!(
            event,
            ConnectorEvent::AttachmentReceived {
                session_id: Some(id),
                bytes: 9,
                ..
            } if id == "s1"
        )));
        let attachments = sink.attachments();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename, "paper.pdf");
        assert_eq!(attachments[0].bytes, b"pdf bytes");
    }

    #[tokio::test]
    async fn save_attachment_adds_extension_from_mime_type() {
        let sink = Arc::new(MemoryImportSink::default());
        let response = router(sink.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/connector/saveAttachment")
                    .header(
                        "X-Metadata",
                        r#"{"sessionID":"s1","title":"paper","mimeType":"application/pdf"}"#,
                    )
                    .body(Body::from("pdf bytes"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(sink.attachments()[0].filename, "paper.pdf");
    }

    #[test]
    fn filename_extension_inference_does_not_duplicate_existing_extension() {
        assert_eq!(
            filename_with_type_extension(
                "paper.pdf",
                Some("application/pdf"),
                None
            ),
            "paper.pdf"
        );
        assert_eq!(
            filename_with_type_extension("snapshot", None, Some("snapshot")),
            "snapshot.html"
        );
    }

    #[tokio::test]
    async fn preflight_allows_connector_headers() {
        let sink = Arc::new(MemoryImportSink::default());
        let response = router(sink)
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/connector/saveItems")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert!(
            response.headers()[ACCESS_CONTROL_ALLOW_HEADERS]
                .to_str()
                .unwrap()
                .contains("x-metadata")
        );
    }

    async fn to_json(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn to_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// Return the current Zotero item type vocabulary covered by import tests.
    fn known_zotero_item_types() -> &'static [&'static str] {
        &[
            "annotation",
            "artwork",
            "attachment",
            "audioRecording",
            "bill",
            "blogPost",
            "book",
            "bookSection",
            "case",
            "computerProgram",
            "conferencePaper",
            "dataset",
            "dictionaryEntry",
            "document",
            "email",
            "encyclopediaArticle",
            "film",
            "forumPost",
            "hearing",
            "instantMessage",
            "interview",
            "journalArticle",
            "letter",
            "magazineArticle",
            "manuscript",
            "map",
            "newspaperArticle",
            "note",
            "patent",
            "podcast",
            "preprint",
            "presentation",
            "radioBroadcast",
            "report",
            "standard",
            "statute",
            "thesis",
            "tvBroadcast",
            "videoRecording",
            "webpage",
        ]
    }
}
