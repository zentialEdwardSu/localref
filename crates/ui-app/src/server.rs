//! Server-rendered Localref web UI.
//!
//! The UI renders through Leptos on top of Axum and talks directly to the
//! daemon facade. Browser forms post back to this router, then redirect to a
//! URL-query state that can be bookmarked or opened from the tray.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::actions::{UiAction, run_action};
use crate::assets::{
    favicon, ui_css, ui_wasm, ui_wasm_bindgen_js, ui_wasm_js,
};
use crate::dto::app_state_from_model;
use crate::state::{UiModel, UiQuery, escape_text, return_path};
use axum::Json;
use axum::Router;
use axum::extract::{Form, Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use localref_core::LocalrefDaemon;
use localref_plugin::discovery::DiscoveredPlugin;
use localref_plugin::manifest::PageMount;
use localref_plugin::state::{PluginUiState, RunOutput};

/// Build the server-rendered UI router for one daemon facade.
pub fn router_with_daemon(daemon: LocalrefDaemon) -> Router {
    router_with_daemon_and_repo_name(daemon, "Localref", Arc::new(Vec::new()))
}

/// Build the server-rendered UI router with a configured repository name.
pub fn router_with_daemon_and_repo_name(
    daemon: LocalrefDaemon,
    repo_name: impl Into<String>,
    plugins: Arc<Vec<DiscoveredPlugin>>,
) -> Router {
    router_with_daemon_repo_plugins_and_context(
        daemon,
        repo_name,
        plugins,
        PluginHostContext::default(),
    )
}

/// Build the UI router with configured plugin host context.
pub fn router_with_daemon_repo_plugins_and_context(
    daemon: LocalrefDaemon,
    repo_name: impl Into<String>,
    plugins: Arc<Vec<DiscoveredPlugin>>,
    plugin_context: PluginHostContext,
) -> Router {
    Router::new()
        .route("/", get(home))
        .route("/ui/state", get(ui_state))
        .route("/assets/favicon.ico", get(favicon))
        .route("/assets/localref-ui.css", get(ui_css))
        .route("/assets/localref-ui.js", get(ui_wasm_js))
        .route("/assets/localref-ui-bindgen.js", get(ui_wasm_bindgen_js))
        .route("/assets/localref-ui-bindgen_bg.wasm", get(ui_wasm))
        .route("/ui/action", post(action))
        .route("/ui/upload", post(upload))
        .route("/plugin/{name}/action", post(plugin_action))
        .route("/plugin/{name}/static/{*path}", get(plugin_static))
        .with_state(ServerState {
            daemon,
            repo_name: repo_name.into(),
            plugins,
            plugin_context,
        })
}

/// Host values passed to plugins during render and action invocations.
#[derive(Clone, Debug, Default)]
pub struct PluginHostContext {
    /// Absolute path to the configured library root.
    pub library_root: PathBuf,
    /// Public REST endpoint configured for browser-side callbacks.
    pub rest_endpoint: String,
}

/// Shared application state for the server-rendered UI.
#[derive(Clone)]
struct ServerState {
    daemon: LocalrefDaemon,
    repo_name: String,
    plugins: Arc<Vec<DiscoveredPlugin>>,
    plugin_context: PluginHostContext,
}

async fn home(
    State(state): State<ServerState>,
    Query(query): Query<UiQuery>,
) -> Response {
    let repo_name = state.repo_name.clone();
    match load_model(&state, query).await {
        Ok(model) => {
            Html(crate::render_page(app_state_from_model(model, repo_name)))
                .into_response()
        }
        Err(error) => Html(format!(
            "<!doctype html><title>{}</title><main><h1>{}</h1><p>{}</p></main>",
            escape_text(&repo_name),
            escape_text(&repo_name),
            escape_text(&error.to_string())
        ))
        .into_response(),
    }
}

async fn ui_state(
    State(state): State<ServerState>,
    Query(query): Query<UiQuery>,
) -> Response {
    let repo_name = state.repo_name.clone();
    match load_model(&state, query).await {
        Ok(model) => {
            Json(app_state_from_model(model, repo_name)).into_response()
        }
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            .into_response(),
    }
}

async fn action(
    State(state): State<ServerState>,
    Form(form): Form<UiAction>,
) -> Redirect {
    let result = run_action(&state.daemon, &form);
    let target = if form.action == "save_rules" {
        rules_action_return(&result)
    } else {
        return_path(&form.return_to)
    };
    Redirect::to(target.as_str())
}

async fn upload(
    State(state): State<ServerState>,
    mut multipart: Multipart,
) -> Response {
    let mut item_id = String::new();
    let mut return_to_value = "/?tab=files".to_string();
    let mut files = Vec::new();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                return (StatusCode::BAD_REQUEST, error.to_string())
                    .into_response();
            }
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "item_id" => {
                item_id = field.text().await.unwrap_or_default();
            }
            "return_to" => {
                return_to_value = field.text().await.unwrap_or_default();
            }
            "file" => {
                let filename = field
                    .file_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| "upload".to_string());
                match field.bytes().await {
                    Ok(bytes) => files.push((filename, bytes.to_vec())),
                    Err(error) => {
                        return (StatusCode::BAD_REQUEST, error.to_string())
                            .into_response();
                    }
                }
            }
            _ => {}
        }
    }
    if !item_id.trim().is_empty() {
        for (filename, bytes) in files {
            if let Err(error) = state
                .daemon
                .add_uploaded_file_to_item(&item_id, &filename, &bytes)
            {
                return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
                    .into_response();
            }
        }
    }
    Redirect::to(return_path(&return_to_value).as_str()).into_response()
}

/// Handle a plugin action submitted from the UI.
async fn plugin_action(
    State(state): State<ServerState>,
    Path(name): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(plugin) = state.plugins.iter().find(|p| p.name() == name) else {
        return (StatusCode::NOT_FOUND, "plugin not found").into_response();
    };

    let action_name = form
        .get("plugin_action")
        .or_else(|| form.get("action"))
        .cloned()
        .unwrap_or_default();
    let return_to =
        form.get("return_to").cloned().unwrap_or_else(|| "/".to_string());

    // Build minimal plugin state for the action.
    let query: UiQuery = return_to
        .split('?')
        .nth(1)
        .map(parse_query_string)
        .unwrap_or_default();
    match load_model(&state, query).await {
        Ok(model) => {
            let plugin_state = build_plugin_ui_state(&model, &state);
            match localref_plugin::invoke::invoke_run(
                &plugin.executable,
                &action_name,
                &form,
                &plugin_state,
            )
            .await
            {
                Ok(output) => {
                    plugin_action_response(&return_to, &action_name, &output)
                }
                Err(error) => {
                    redirect_with_plugin_error(&return_to, &error.to_string())
                }
            }
        }
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            .into_response(),
    }
}

/// Load the UI model and render active plugin tab content when needed.
async fn load_model(
    state: &ServerState,
    query: UiQuery,
) -> localref_core::error::Result<UiModel> {
    let mut model = UiModel::load(&state.daemon, query, &state.plugins)?;
    render_fixed_plugin_slots(&mut model, state).await;
    if let Some((plugin, page_id)) = active_plugin_page(&model, &state.plugins)
    {
        let plugin_state = build_plugin_ui_state(&model, state);
        let mut html = match localref_plugin::invoke::invoke_render(
            &plugin.executable,
            page_id,
            &plugin_state,
        )
        .await
        {
            Ok(output) if output.status == "ok" => output.html,
            Ok(output) => {
                let message = output
                    .message
                    .unwrap_or_else(|| "plugin render failed".to_string());
                plugin_error_html(&message)
            }
            Err(error) => plugin_error_html(&error.to_string()),
        };
        if let Some(message) = model.query.plugin_error.as_deref() {
            html = format!("{}{html}", plugin_error_html(message));
        }
        model.plugin_page_html = Some(html);
    }
    Ok(model)
}

/// Render plugin pages mounted into fixed host UI slots.
async fn render_fixed_plugin_slots(model: &mut UiModel, state: &ServerState) {
    let target_mount = if model.selected_ids.is_empty() {
        PageMount::MetadataPage
    } else {
        PageMount::SelectionPage
    };
    let plugin_state = build_plugin_ui_state(model, state);
    for plugin in state.plugins.iter() {
        for page in plugin
            .manifest
            .pages
            .iter()
            .filter(|page| page.mount == target_mount)
        {
            let html = match localref_plugin::invoke::invoke_render(
                &plugin.executable,
                &page.id,
                &plugin_state,
            )
            .await
            {
                Ok(output) if output.status == "ok" => output.html,
                Ok(output) => plugin_error_html(
                    output
                        .message
                        .as_deref()
                        .unwrap_or("plugin render failed"),
                ),
                Err(error) => plugin_error_html(&error.to_string()),
            };
            model.plugin_slots.push(crate::model::PluginSlotHtml {
                mount: page_mount_name(&page.mount).to_string(),
                plugin_name: plugin.name().to_string(),
                page_id: page.id.clone(),
                label: page.label.clone(),
                html,
            });
        }
    }
}

/// Return the stable JSON name for a plugin page mount.
fn page_mount_name(mount: &PageMount) -> &'static str {
    match mount {
        PageMount::DetailTab => "detail_tab",
        PageMount::MetadataPage => "metadata_page",
        PageMount::SelectionPage => "selection_page",
    }
}

/// Return the plugin and page id for the active plugin tab.
fn active_plugin_page<'a>(
    model: &UiModel,
    plugins: &'a [DiscoveredPlugin],
) -> Option<(&'a DiscoveredPlugin, &'a str)> {
    plugin_page_from_tab(&model.tab, plugins)
}

fn plugin_page_from_tab<'a>(
    tab: &str,
    plugins: &'a [DiscoveredPlugin],
) -> Option<(&'a DiscoveredPlugin, &'a str)> {
    let rest = tab.strip_prefix("plugin:")?;
    let (plugin_name, page_id) = rest.split_once(':')?;
    plugins.iter().find_map(|plugin| {
        let page =
            plugin.manifest.pages.iter().find(|page| page.id == page_id)?;
        (plugin.name() == plugin_name).then_some((plugin, page.id.as_str()))
    })
}

/// Build an escaped plugin error fragment for the detail pane.
fn plugin_error_html(message: &str) -> String {
    format!(
        r#"<div class="plugin-error" role="alert"><h3>Plugin error</h3><p>{}</p></div>"#,
        escape_text(message)
    )
}

/// Convert a plugin action result into the redirect visible to the user.
fn plugin_action_response(
    return_to: &str,
    action_name: &str,
    output: &RunOutput,
) -> Response {
    plugin_action_response_with_picker(
        return_to,
        action_name,
        output,
        |filename| {
            native_win32::save_file_path(filename)
                .map_err(|error| error.to_string())
        },
    )
}

fn plugin_action_response_with_picker(
    return_to: &str,
    action_name: &str,
    output: &RunOutput,
    save_path: impl FnOnce(&str) -> Result<Option<std::path::PathBuf>, String>,
) -> Response {
    if output.status == "ok" {
        if let Some(result) = output.result.as_deref() {
            return plugin_save_response(
                return_to,
                action_name,
                output,
                result,
                save_path,
            );
        }
        return Redirect::to(return_to).into_response();
    }
    let message = output.message.as_deref().unwrap_or("plugin action failed");
    redirect_with_plugin_error(return_to, message)
}

fn plugin_save_response(
    return_to: &str,
    action_name: &str,
    output: &RunOutput,
    result: &str,
    save_path: impl FnOnce(&str) -> Result<Option<std::path::PathBuf>, String>,
) -> Response {
    let filename = output
        .filename
        .as_deref()
        .map(safe_download_filename)
        .unwrap_or_else(|| default_download_filename(action_name));
    let path = match save_path(&filename) {
        Ok(Some(path)) => path,
        Ok(None) => return Redirect::to(return_to).into_response(),
        Err(message) => {
            return redirect_with_plugin_error(return_to, &message);
        }
    };
    match std::fs::write(&path, result.as_bytes()) {
        Ok(()) => Redirect::to(return_to).into_response(),
        Err(error) => redirect_with_plugin_error(
            return_to,
            &format!("failed to save {}: {error}", path.display()),
        ),
    }
}

fn default_download_filename(action_name: &str) -> String {
    format!("localref-{}.txt", safe_download_filename(action_name))
}

fn safe_download_filename(value: &str) -> String {
    let mut safe = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            safe.push(ch);
        } else if ch.is_whitespace() {
            safe.push('-');
        }
    }
    if safe.is_empty() { "localref-export.txt".to_string() } else { safe }
}

fn redirect_with_plugin_error(return_to: &str, message: &str) -> Response {
    let error_param = encode_query_component(message);
    Redirect::to(&append_query_param(
        return_to,
        &format!("plugin_error={error_param}"),
    ))
    .into_response()
}

fn append_query_param(path: &str, param: &str) -> String {
    let separator = if path.contains('?') { '&' } else { '?' };
    format!("{path}{separator}{param}")
}

/// Serve a static file from a plugin's `static/` directory.
async fn plugin_static(
    Path((name, path)): Path<(String, String)>,
    State(state): State<ServerState>,
) -> Response {
    let Some(plugin) = state.plugins.iter().find(|p| p.name() == name) else {
        return (StatusCode::NOT_FOUND, "plugin not found").into_response();
    };
    let file_path = plugin.static_dir.join(&path);
    // Security: prevent directory traversal.
    let Ok(canonical) = file_path.canonicalize() else {
        return (StatusCode::NOT_FOUND, "file not found").into_response();
    };
    let Ok(static_canonical) = plugin.static_dir.canonicalize() else {
        return (StatusCode::NOT_FOUND, "file not found").into_response();
    };
    if !canonical.starts_with(&static_canonical) {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    match tokio::fs::read(&canonical).await {
        Ok(bytes) => {
            let content_type =
                mime_guess::from_path(&canonical).first_or_octet_stream();
            (
                [(axum::http::header::CONTENT_TYPE, content_type.as_ref())],
                bytes,
            )
                .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "file not found").into_response(),
    }
}

/// Build plugin UI state from the current server model.
fn build_plugin_ui_state(
    model: &UiModel,
    server_state: &ServerState,
) -> PluginUiState {
    use localref_plugin::state::{
        PluginActiveDetail, PluginCategorySummary, PluginItemSummary,
        PluginUiState,
    };

    PluginUiState {
        repo_name: server_state.repo_name.clone(),
        search: model.query.q.clone(),
        category: model.query.category.clone(),
        items: model
            .items
            .iter()
            .map(|item| PluginItemSummary {
                id: item.id.clone(),
                title: item.title.clone(),
                authors: item.authors.clone(),
                item_type: item.item_type.clone(),
                categories: item.categories.clone(),
                main_file: item.main_file.clone(),
                files: {
                    let mut paths: Vec<String> = Vec::new();
                    if let Some(ref main) = item.main_file {
                        paths.push(main.clone());
                    }
                    paths.extend(item.extra_files.clone());
                    paths
                },
            })
            .collect(),
        categories: model
            .categories
            .iter()
            .map(|c| PluginCategorySummary {
                path: c.path.clone(),
                item_count: c.item_ids.len(),
            })
            .collect(),
        selected_ids: model.selected_ids.clone(),
        active_id: model.active_id.clone(),
        active_detail: model.active_metadata.as_ref().map(|doc| {
            PluginActiveDetail {
                metadata_revision: doc.metadata_revision.clone(),
                title: doc.metadata.title.clone(),
                authors: crate::state::author_summary(&doc.metadata),
                item_type: doc.metadata.item_type.clone(),
                year: doc.metadata.year,
                doi: doc.metadata.doi.clone(),
                venue: doc.metadata.venue.clone(),
                language: doc.metadata.language.clone(),
                uri: doc.metadata.uri.clone(),
                abstract_note: doc.metadata.abstract_note.clone(),
            }
        }),
        tab: model.tab.clone(),
        status_label: model.status_label(),
        library_root: server_state
            .plugin_context
            .library_root
            .to_string_lossy()
            .into_owned(),
        rest_endpoint: server_state.plugin_context.rest_endpoint.clone(),
    }
}

/// Parse URL query string into key-value pairs.
fn parse_query_string(query: &str) -> UiQuery {
    let mut result = UiQuery::default();
    for part in query.split('&') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let decoded = decode_query_value(value);
        match key {
            "q" => result.q = decoded,
            "category" => result.category = decoded,
            "active" => result.active = decoded,
            "selected" => result.selected = decoded,
            "tab" => result.tab = decoded,
            "plugin" => result.plugin = decoded,
            "plugin_error" => result.plugin_error = decoded,
            _ => {}
        }
    }
    result
}

fn decode_query_value(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let high = input.next().and_then(hex_value)?;
                let low = input.next().and_then(hex_value)?;
                bytes.push((high << 4) | low);
            }
            byte => bytes.push(byte),
        }
    }
    String::from_utf8(bytes).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn rules_action_return(result: &Result<(), String>) -> String {
    match result {
        Ok(()) => "/?tab=rules&rules_status=saved".to_string(),
        Err(error) => {
            format!("/?tab=rules&rules_error={}", encode_query(error))
        }
    }
}

fn encode_query(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
        {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Percent-encode a string for use in a query parameter value.
fn encode_query_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
        {
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
    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::Request;
    use localref_core::storage::CategorySummary;
    use localref_core::types::CategoryPath;
    use localref_plugin::manifest::{PageMount, PageSpec, PluginManifest};
    use std::path::PathBuf;
    use tower::ServiceExt;

    #[test]
    fn plugin_tab_selects_declared_plugin_page() {
        let plugins = vec![DiscoveredPlugin {
            dir: PathBuf::from("plugins/bibtexer"),
            manifest: PluginManifest {
                name: "bibtexer".to_string(),
                executable: None,
                description: None,
                actions: Vec::new(),
                pages: vec![PageSpec {
                    id: "export_form".to_string(),
                    label: "Export".to_string(),
                    mount: PageMount::DetailTab,
                    route: "export".to_string(),
                }],
            },
            executable: PathBuf::from("plugins/bibtexer/bibtexer"),
            static_dir: PathBuf::from("plugins/bibtexer/static"),
        }];

        let Some((plugin, page_id)) =
            plugin_page_from_tab("plugin:bibtexer:export_form", &plugins)
        else {
            panic!("plugin tab should map to the declared page");
        };

        assert_eq!(plugin.name(), "bibtexer");
        assert_eq!(page_id, "export_form");
        assert!(
            plugin_page_from_tab("plugin:bibtexer:missing", &plugins)
                .is_none()
        );
    }

    #[test]
    fn plugin_action_error_status_redirects_with_error_message() {
        let response = plugin_action_response(
            "/?tab=plugin:bibtexer:export_form",
            "export_bibtex",
            &RunOutput::error("no items selected"),
        );

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get("location").and_then(|h| h.to_str().ok()),
            Some(
                "/?tab=plugin:bibtexer:export_form&plugin_error=no%20items%20selected"
            )
        );
    }

    #[test]
    fn plugin_action_result_saves_file_content() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("export.bib");

        let response = plugin_action_response_with_picker(
            "/?tab=plugin:bibtexer:export_form",
            "export_bibtex",
            &RunOutput::ok("@article{demo}")
                .content_type("text/x-bibtex")
                .filename("localref-export.bib"),
            |_| Ok(Some(path.clone())),
        );

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get("location").and_then(|h| h.to_str().ok()),
            Some("/?tab=plugin:bibtexer:export_form")
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "@article{demo}");
    }

    #[tokio::test]
    async fn renders_dashboard_and_category_form() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert!(response.status().is_success());
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Localref"));
        assert!(html.contains("Run Scan"));
        assert!(html.contains("Watcher On"));
        assert!(html.contains("Watcher Paused"));
        assert!(html.contains(r#"class="watcher-form""#));
        assert!(html.contains(r#"data-route-action="true""#));
        assert!(html.contains(r#"type="radio""#));
        assert!(html.contains("Apply Watcher"));
        assert!(html.contains("library-search"));
        assert!(html.contains("library-category"));
        assert!(html.contains("<link"));
        assert!(html.contains(r#"rel="icon""#));
        assert!(html.contains(r#"href="/assets/favicon.ico""#));
        assert!(html.contains(r#"type="image/x-icon""#));
        assert!(html.contains(r#"rel="stylesheet""#));
        assert!(html.contains(r#"href="/assets/localref-ui.css""#));
        assert!(!html.contains("<style>"));
        assert!(html.contains("Create Category"));
        assert!(html.contains("Current"));
        assert!(html.contains("Available"));
        assert!(html.contains("Current Categories:"));
        assert!(html.contains(r#"src="/assets/localref-ui.js""#));
        assert!(!html.contains("filterRouteFrom"));
        assert!(!html.contains("document.querySelectorAll('.library-row')"));
        assert!(!html.contains("cdn.tailwindcss.com"));
        assert!(!html.contains("Update Selection"));

        let events_index = html.find(">Events</button>").unwrap();
        let metadata_index = html.find(">Metadata</a>").unwrap();
        let files_index = html.find(">Files</a>").unwrap();
        let editor_index = html.find("Current Categories:").unwrap();
        let create_index = html.find("Create Category").unwrap();
        assert!(events_index < metadata_index);
        assert!(metadata_index < files_index);
        assert!(editor_index < create_index);
    }

    #[tokio::test]
    async fn renders_configured_repository_name() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon_and_repo_name(
            daemon,
            "Research Vault",
            Arc::new(Vec::new()),
        );

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert!(response.status().is_success());
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("<title>Research Vault</title>"));
        assert!(html.contains("<h1>Research Vault</h1>"));
        assert!(html.contains(r#""repo_name":"Research Vault""#));
    }

    #[tokio::test]
    async fn serves_favicon_from_assets_folder() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/assets/favicon.ico")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.status().is_success());
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            headers.get("content-type").and_then(|value| value.to_str().ok()),
            Some("image/x-icon")
        );
        assert!(body.starts_with(&[0, 0, 1, 0]));
    }

    #[tokio::test]
    async fn serves_compiled_tailwind_css_from_assets_folder() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/assets/localref-ui.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.status().is_success());
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let css = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(
            headers.get("content-type").and_then(|value| value.to_str().ok()),
            Some("text/css; charset=utf-8")
        );
        assert!(css.contains("tailwindcss v4."));
        assert!(css.contains(".app-shell"));
    }

    #[tokio::test]
    async fn serves_wasm_bootstrap_assets_from_assets_folder() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let script_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/localref-ui.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let script_headers = script_response.headers().clone();
        let script = String::from_utf8(
            to_bytes(script_response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            script_headers
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("text/javascript; charset=utf-8")
        );
        assert!(script.contains("import init"));
        assert!(script.contains("await init()"));
        assert!(script.contains("localref-ui-bindgen.js"));
        assert!(!script.contains("filterRouteFrom"));

        let bindgen_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/localref-ui-bindgen.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bindgen = String::from_utf8(
            to_bytes(bindgen_response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(bindgen.contains("localref-ui-bindgen_bg.wasm"));

        let wasm_response = app
            .oneshot(
                Request::builder()
                    .uri("/assets/localref-ui-bindgen_bg.wasm")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let wasm_headers = wasm_response.headers().clone();
        let wasm =
            to_bytes(wasm_response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            wasm_headers
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/wasm")
        );
        assert!(wasm.starts_with(b"\0asm"));
    }

    #[tokio::test]
    async fn renders_events_as_toggle_panel_without_navigation_link() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("<button"));
        assert!(html.contains(r#"class="top-link""#));
        assert!(html.contains(r#"data-events-toggle="true""#));
        assert!(html.contains(r#"aria-pressed="false""#));
        assert!(html.contains(">Events</button>"));
        assert!(html.contains(r#"class="event-panel""#));
        assert!(html.contains("hidden"));
        assert!(html.contains("data-primary-detail"));
        assert!(html.contains("data-primary-detail-head"));
        assert!(!html.contains(r#">Events</a>"#));
    }

    #[tokio::test]
    async fn renders_document_navigation_as_client_router_controls() {
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-one".to_string()),
                    uri: None,
                    connector_item_id: Some("one".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Router Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Router Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(r#"data-route-active="lr:zotero:one""#));
        assert!(html.contains(r#"data-route-tab="metadata""#));
        assert!(html.contains(r#"data-route-tab="files""#));
        assert!(html.contains(r#"src="/assets/localref-ui.js""#));
        assert!(!html.contains("history.pushState"));
        assert!(!html.contains("fetch(routeUrl"));
        assert!(!html.contains(r#"<a class="item-link""#));
        assert!(!html.contains(r#"<a class="right-tab"#));
    }

    #[tokio::test]
    async fn renders_category_changes_as_client_router_actions() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon.create_category(CategoryPath::new("Inbox").unwrap()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(r#"data-route-action="true""#));
        assert!(html.contains(r#"value="create_category""#));
        assert!(html.contains(r#"value="add_category""#));
        assert!(html.contains(r#"src="/assets/localref-ui.js""#));
        assert!(!html.contains("addEventListener('submit'"));
        assert!(!html.contains("form.getAttribute('action') || form.action"));
        assert!(!html.contains("fetch(actionUrl"));
        assert!(!html.contains("keepCategoryEditorOpen"));
        assert!(!html.contains("setAttribute('open', '')"));
        assert!(!html.contains("new URLSearchParams(new FormData(form))"));
        assert!(!html.contains("body: new FormData(form)"));
    }

    #[tokio::test]
    async fn renders_checked_selection_as_bulk_category_editor() {
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        for suffix in ["one", "two"] {
            daemon
                .import_connector_item(ConnectorImport {
                    item: ConnectorItem {
                        session_id: Some(format!("session-{suffix}")),
                        uri: None,
                        connector_item_id: Some(suffix.to_string()),
                        item_type: Some("journalArticle".to_string()),
                        title: format!("Paper {suffix}"),
                        abstract_note: None,
                        doi: None,
                        raw: json!({"title": format!("Paper {suffix}")}),
                    },
                    attachments: Vec::new(),
                })
                .unwrap();
        }
        daemon.create_category(CategoryPath::new("Common").unwrap()).unwrap();
        daemon.create_category(CategoryPath::new("OnlyOne").unwrap()).unwrap();
        daemon
            .add_items_category(
                &["lr:zotero:one".to_string(), "lr:zotero:two".to_string()],
                CategoryPath::new("Common").unwrap(),
            )
            .unwrap();
        daemon
            .add_item_category(
                "lr:zotero:one",
                CategoryPath::new("OnlyOne").unwrap(),
            )
            .unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/?selected=lr:zotero:one,lr:zotero:two&tab=metadata")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Selected"));
        assert!(html.contains("items</h2>"));
        assert!(!html.contains(">Metadata</button>"));
        assert!(!html.contains(">Files</button>"));
        assert!(html.contains("<h2>Selected 2 items</h2>"));
        assert!(html.contains(r#"<span class="category-tag">Common</span>"#));
        assert!(html.contains(r#"value="OnlyOne""#));
        assert!(
            !html.contains(r#"<span class="category-tag">OnlyOne</span>"#)
        );
        assert!(!html.contains("Save Metadata"));
        assert!(!html.contains(r#"name="expected_revision""#));
    }

    #[tokio::test]
    async fn renders_selection_form_as_client_router_control() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(r#"class="selection-form""#));
        assert!(html.contains(r#"src="/assets/localref-ui.js""#));
        assert!(!html.contains("formData.getAll('item')"));
        assert!(!html.contains("params.set('selected', selected.join(','))"));
        assert!(!html.contains("scheduleFilterRoute"));
        assert!(!html.contains("routeUrl.searchParams.delete('selected')"));
        assert!(!html.contains("visitRoute(selectionRouteFrom(form), true)"));
    }

    #[tokio::test]
    async fn filters_items_by_search_and_category_query() {
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        for (suffix, title) in
            [("alpha", "Alpha Search Paper"), ("beta", "Beta Category Paper")]
        {
            daemon
                .import_connector_item(ConnectorImport {
                    item: ConnectorItem {
                        session_id: Some(format!("session-{suffix}")),
                        uri: None,
                        connector_item_id: Some(suffix.to_string()),
                        item_type: Some("journalArticle".to_string()),
                        title: title.to_string(),
                        abstract_note: None,
                        doi: None,
                        raw: json!({"title": title}),
                    },
                    attachments: Vec::new(),
                })
                .unwrap();
        }
        daemon.create_category(CategoryPath::new("Topic/A").unwrap()).unwrap();
        daemon.create_category(CategoryPath::new("Topic/B").unwrap()).unwrap();
        daemon
            .add_item_category(
                "lr:zotero:alpha",
                CategoryPath::new("Topic/A").unwrap(),
            )
            .unwrap();
        daemon
            .add_item_category(
                "lr:zotero:beta",
                CategoryPath::new("Topic/B").unwrap(),
            )
            .unwrap();
        let app = router_with_daemon(daemon);

        let search_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/?q=alpha")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let search_html = String::from_utf8(
            to_bytes(search_response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(search_html.contains("Alpha Search Paper"));
        assert!(
            !search_html.contains(r#"data-route-active="lr:zotero:beta""#)
        );

        let category_response = app
            .oneshot(
                Request::builder()
                    .uri("/?category=Topic%2FB")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let category_html = String::from_utf8(
            to_bytes(category_response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(
            !category_html.contains(r#"data-route-active="lr:zotero:alpha""#)
        );
        assert!(category_html.contains("Beta Category Paper"));
    }

    #[tokio::test]
    async fn ui_state_filters_items_and_reports_route_state() {
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        for (suffix, title) in
            [("alpha", "Alpha State Paper"), ("beta", "Beta State Paper")]
        {
            daemon
                .import_connector_item(ConnectorImport {
                    item: ConnectorItem {
                        session_id: Some(format!("session-{suffix}")),
                        uri: None,
                        connector_item_id: Some(suffix.to_string()),
                        item_type: Some("journalArticle".to_string()),
                        title: title.to_string(),
                        abstract_note: None,
                        doi: None,
                        raw: json!({"title": title}),
                    },
                    attachments: Vec::new(),
                })
                .unwrap();
        }
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ui/state?q=alpha&active=lr:zotero:beta&tab=files")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.status().is_success());
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let state: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(state["items"].as_array().unwrap().len(), 1);
        assert_eq!(state["items"][0]["title"], "Alpha State Paper");
        assert_eq!(state["active_id"], "lr:zotero:alpha");
        assert_eq!(state["tab"], "files");
        assert_eq!(
            state["return_to"],
            "/?q=alpha&active=lr:zotero:alpha&tab=files"
        );
        assert_eq!(state["active_detail"]["title"], "Alpha State Paper");
        assert_eq!(state["active_detail"]["item_type"], "journalArticle");
        assert_eq!(state["active_detail"]["authors"], "");
    }

    #[test]
    fn ui_model_filters_items_by_query_state() {
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        for (suffix, title) in
            [("alpha", "Alpha Search Paper"), ("beta", "Beta Category Paper")]
        {
            daemon
                .import_connector_item(ConnectorImport {
                    item: ConnectorItem {
                        session_id: Some(format!("session-{suffix}")),
                        uri: None,
                        connector_item_id: Some(suffix.to_string()),
                        item_type: Some("journalArticle".to_string()),
                        title: title.to_string(),
                        abstract_note: None,
                        doi: None,
                        raw: json!({"title": title}),
                    },
                    attachments: Vec::new(),
                })
                .unwrap();
        }

        let model = UiModel::load(
            &daemon,
            UiQuery { q: Some("alpha".to_string()), ..UiQuery::default() },
            &Arc::new(Vec::new()),
        )
        .unwrap();

        assert_eq!(model.items.len(), 1);
        assert_eq!(model.items[0].id, "lr:zotero:alpha");
    }

    #[tokio::test]
    async fn renders_double_click_open_data_for_openable_attachment() {
        use localref_core::types::{
            ConnectorAttachment, ConnectorImport, ConnectorItem,
        };
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-open".to_string()),
                    uri: None,
                    connector_item_id: Some("open".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Openable Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Openable Paper"}),
                },
                attachments: vec![ConnectorAttachment {
                    session_id: Some("session-open".to_string()),
                    parent_item_id: Some("open".to_string()),
                    title: Some("PDF".to_string()),
                    filename: "paper.pdf".to_string(),
                    mime_type: Some("application/pdf".to_string()),
                    bytes: b"pdf".to_vec(),
                    raw_metadata: None,
                }],
            })
            .unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(r#"data-open-file="paper.pdf""#));
        assert!(html.contains(r#"src="/assets/localref-ui.js""#));
        assert!(!html.contains("addEventListener('dblclick'"));
        assert!(!html.contains("openItemFile(button)"));
    }

    #[tokio::test]
    async fn category_router_action_updates_active_item_without_selection() {
        use axum::http::header::CONTENT_TYPE;
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-active".to_string()),
                    uri: None,
                    connector_item_id: Some("active".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Active Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Active Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();
        daemon.create_category(CategoryPath::new("Inbox").unwrap()).unwrap();
        let app = router_with_daemon(daemon.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ui/action")
                    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "action=add_category&return_to=%2F%3Factive%3Dlr%3Azotero%3Aactive%26tab%3Dmetadata&category=Inbox",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.status().is_redirection());
        let item = daemon.get_item("lr:zotero:active").unwrap().unwrap();
        assert_eq!(item.categories, vec!["Inbox"]);
    }

    #[tokio::test]
    async fn renders_local_file_actions_on_files_tab() {
        use localref_core::types::{
            ConnectorAttachment, ConnectorImport, ConnectorItem,
        };
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-files".to_string()),
                    uri: None,
                    connector_item_id: Some("files".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Files Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Files Paper"}),
                },
                attachments: vec![ConnectorAttachment {
                    session_id: Some("session-files".to_string()),
                    parent_item_id: Some("files".to_string()),
                    title: Some("PDF".to_string()),
                    filename: "paper.pdf".to_string(),
                    mime_type: Some("application/pdf".to_string()),
                    bytes: b"pdf".to_vec(),
                    raw_metadata: None,
                }],
            })
            .unwrap();
        daemon
            .add_uploaded_file_to_item(
                "lr:zotero:files",
                "notes.txt",
                b"notes",
            )
            .unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/?active=lr:zotero:files&tab=files")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Open Folder"));
        assert!(html.contains("Add Files"));
        assert!(html.contains("Drop files here"));
        assert!(html.contains(r#"action="/ui/upload""#));
        assert!(html.contains(r#"type="file""#));
        assert!(html.contains("Main"));
        assert!(html.contains("Set Main"));
        assert!(html.contains("main .pdf"));
        assert!(!html.contains("Local File Path"));
        assert!(!html.contains("Import Path"));
        assert!(!html.contains("Import File"));
    }

    #[tokio::test]
    async fn upload_route_adds_selected_file_to_active_item() {
        use axum::http::header::CONTENT_TYPE;
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-upload".to_string()),
                    uri: None,
                    connector_item_id: Some("upload".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Upload Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Upload Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();
        let app = router_with_daemon(daemon.clone());
        let body = concat!(
            "--LOCALREF\r\n",
            "Content-Disposition: form-data; name=\"item_id\"\r\n\r\n",
            "lr:zotero:upload\r\n",
            "--LOCALREF\r\n",
            "Content-Disposition: form-data; name=\"return_to\"\r\n\r\n",
            "/?active=lr:zotero:upload&tab=files\r\n",
            "--LOCALREF\r\n",
            "Content-Disposition: form-data; name=\"file\"; filename=\"notes.txt\"\r\n",
            "Content-Type: text/plain\r\n\r\n",
            "notes\r\n",
            "--LOCALREF--\r\n",
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ui/upload")
                    .header(
                        CONTENT_TYPE,
                        "multipart/form-data; boundary=LOCALREF",
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.status().is_redirection());
        let files = daemon.item_files("lr:zotero:upload").unwrap().unwrap();
        assert!(files.files.iter().any(|file| file.path == "notes.txt"));
        let item = daemon.get_item("lr:zotero:upload").unwrap().unwrap();
        assert_eq!(item.main_file.as_deref(), Some("notes.txt"));
    }

    #[tokio::test]
    async fn renders_rules_editor_tab() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".localref")).unwrap();
        std::fs::write(
            temp.path().join(".localref").join("rules.toml"),
            "[[rules]]\nname = \"RIS\"\ntarget = \"Wireless/RIS\"\nquery = 'title:RIS'\n",
        )
        .unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/?tab=rules")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(r#"data-route-tab="rules""#));
        assert!(html.contains("<h2>Rules Editor</h2>"));
        assert!(html.contains(r#"name="rules_text""#));
        assert!(html.contains("[[rules]]"));
        assert!(html.contains("Save Rules"));
    }

    #[tokio::test]
    async fn saving_rules_redirects_to_parsed_result_dialog() {
        use axum::http::header::CONTENT_TYPE;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ui/action")
                    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "action=save_rules&return_to=%2F%3Ftab%3Drules&rules_text=%5B%5Brules%5D%5D%0Aname%20%3D%20%22RIS%22%0Atarget%20%3D%20%22Wireless%2FRIS%22%0Aquery%20%3D%20%27title%3ARIS%27%0A",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.status().is_redirection());
        assert_eq!(
            response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok()),
            Some("/?tab=rules&rules_status=saved")
        );
    }

    #[tokio::test]
    async fn renders_saved_rules_result_dialog() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".localref")).unwrap();
        std::fs::write(
            temp.path().join(".localref").join("rules.toml"),
            "[[rules]]\nname = \"RIS\"\ntarget = \"Wireless/RIS\"\nquery = 'title:RIS'\n",
        )
        .unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/?tab=rules&rules_status=saved")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(r#"class="rules-result-dialog""#));
        assert!(html.contains("Parsed Rules"));
        assert!(html.contains("RIS"));
        assert!(html.contains("Wireless/RIS"));
        assert!(html.contains("title:RIS"));
    }

    #[tokio::test]
    async fn invalid_rules_redirect_to_error_dialog() {
        use axum::http::header::CONTENT_TYPE;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ui/action")
                    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "action=save_rules&return_to=%2F%3Ftab%3Drules&rules_text=%5B%5Brules%5D%5D%0Aname%20%3D%20%22Bad%22%0Atarget%20%3D%20%22Wireless%2FRIS%22%0Aquery%20%3D%20%27broken%27%0A",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.status().is_redirection());
        let location =
            response.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.starts_with("/?tab=rules&rules_error="));
    }

    #[tokio::test]
    async fn renders_rules_error_dialog() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/?tab=rules&rules_error=rule%20query%20atoms%20must%20use%20field%3Apattern")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(r#"class="rules-result-dialog is-error""#));
        assert!(html.contains("Rules Error"));
        assert!(html.contains("rule query atoms must use field:pattern"));
    }

    #[test]
    fn category_action_updates_all_selected_items() {
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        for suffix in ["one", "two"] {
            daemon
                .import_connector_item(ConnectorImport {
                    item: ConnectorItem {
                        session_id: Some(format!("session-{suffix}")),
                        uri: None,
                        connector_item_id: Some(suffix.to_string()),
                        item_type: Some("journalArticle".to_string()),
                        title: format!("Paper {suffix}"),
                        abstract_note: None,
                        doi: None,
                        raw: json!({"title": format!("Paper {suffix}")}),
                    },
                    attachments: Vec::new(),
                })
                .unwrap();
        }
        daemon.create_category(CategoryPath::new("Inbox").unwrap()).unwrap();

        run_action(
            &daemon,
            &UiAction {
                action: "add_category".to_string(),
                return_to:
                    "/?selected=lr:zotero:one,lr:zotero:two&tab=metadata"
                        .to_string(),
                category: Some("Inbox".to_string()),
                ..UiAction::default()
            },
        )
        .unwrap();

        let first = daemon.get_item("lr:zotero:one").unwrap().unwrap();
        let second = daemon.get_item("lr:zotero:two").unwrap().unwrap();
        assert_eq!(first.categories, vec!["Inbox"]);
        assert_eq!(second.categories, vec!["Inbox"]);
    }

    #[test]
    fn category_action_uses_active_item_when_no_checkboxes_are_selected() {
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-active".to_string()),
                    uri: None,
                    connector_item_id: Some("active".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Active Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Active Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();
        daemon.create_category(CategoryPath::new("Inbox").unwrap()).unwrap();

        run_action(
            &daemon,
            &UiAction {
                action: "add_category".to_string(),
                return_to: "/?active=lr:zotero:active&tab=metadata"
                    .to_string(),
                category: Some("Inbox".to_string()),
                ..UiAction::default()
            },
        )
        .unwrap();

        let item = daemon.get_item("lr:zotero:active").unwrap().unwrap();
        assert_eq!(item.categories, vec!["Inbox"]);
    }

    #[test]
    fn category_action_completes_partially_existing_selected_category() {
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        for suffix in ["one", "two"] {
            daemon
                .import_connector_item(ConnectorImport {
                    item: ConnectorItem {
                        session_id: Some(format!("session-{suffix}")),
                        uri: None,
                        connector_item_id: Some(suffix.to_string()),
                        item_type: Some("journalArticle".to_string()),
                        title: format!("Paper {suffix}"),
                        abstract_note: None,
                        doi: None,
                        raw: json!({"title": format!("Paper {suffix}")}),
                    },
                    attachments: Vec::new(),
                })
                .unwrap();
        }
        daemon.create_category(CategoryPath::new("OnlyOne").unwrap()).unwrap();
        daemon
            .add_item_category(
                "lr:zotero:one",
                CategoryPath::new("OnlyOne").unwrap(),
            )
            .unwrap();

        run_action(
            &daemon,
            &UiAction {
                action: "add_category".to_string(),
                return_to:
                    "/?selected=lr:zotero:one,lr:zotero:two&tab=metadata"
                        .to_string(),
                category: Some("OnlyOne".to_string()),
                ..UiAction::default()
            },
        )
        .unwrap();

        let first = daemon.get_item("lr:zotero:one").unwrap().unwrap();
        let second = daemon.get_item("lr:zotero:two").unwrap().unwrap();
        assert_eq!(first.categories, vec!["OnlyOne"]);
        assert_eq!(second.categories, vec!["OnlyOne"]);
    }

    #[test]
    fn category_action_decodes_browser_encoded_selected_return_to() {
        use localref_core::types::{ConnectorImport, ConnectorItem};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        for suffix in ["one", "two"] {
            daemon
                .import_connector_item(ConnectorImport {
                    item: ConnectorItem {
                        session_id: Some(format!("session-{suffix}")),
                        uri: None,
                        connector_item_id: Some(suffix.to_string()),
                        item_type: Some("journalArticle".to_string()),
                        title: format!("Paper {suffix}"),
                        abstract_note: None,
                        doi: None,
                        raw: json!({"title": format!("Paper {suffix}")}),
                    },
                    attachments: Vec::new(),
                })
                .unwrap();
        }
        daemon.create_category(CategoryPath::new("Encoded").unwrap()).unwrap();

        run_action(
            &daemon,
            &UiAction {
                action: "add_category".to_string(),
                return_to:
                    "/?selected=lr%3Azotero%3Aone%2Clr%3Azotero%3Atwo&tab=metadata"
                        .to_string(),
                category: Some("Encoded".to_string()),
                ..UiAction::default()
            },
        )
        .unwrap();

        let first = daemon.get_item("lr:zotero:one").unwrap().unwrap();
        let second = daemon.get_item("lr:zotero:two").unwrap().unwrap();
        assert_eq!(first.categories, vec!["Encoded"]);
        assert_eq!(second.categories, vec!["Encoded"]);
    }

    #[test]
    fn save_rules_action_updates_rules_file() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        run_action(
            &daemon,
            &UiAction {
                action: "save_rules".to_string(),
                return_to: "/?tab=rules".to_string(),
                rules_text: Some(
                    "[[rules]]\nname = \"RIS\"\ntarget = \"Wireless/RIS\"\nquery = 'title:RIS'\n"
                        .to_string(),
                ),
                ..UiAction::default()
            },
        )
        .unwrap();

        let rules_path = temp.path().join(".localref").join("rules.toml");
        let saved = std::fs::read_to_string(rules_path).unwrap();
        assert!(saved.contains("target = \"Wireless/RIS\""));
    }

    #[test]
    fn set_main_file_preserves_previous_main_as_extra_file() {
        use localref_core::types::{
            ConnectorAttachment, ConnectorImport, ConnectorItem,
        };
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-main".to_string()),
                    uri: None,
                    connector_item_id: Some("main".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Main Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Main Paper"}),
                },
                attachments: vec![ConnectorAttachment {
                    session_id: Some("session-main".to_string()),
                    parent_item_id: Some("main".to_string()),
                    title: Some("PDF".to_string()),
                    filename: "paper.pdf".to_string(),
                    mime_type: Some("application/pdf".to_string()),
                    bytes: b"pdf".to_vec(),
                    raw_metadata: None,
                }],
            })
            .unwrap();
        daemon
            .add_uploaded_file_to_item("lr:zotero:main", "notes.txt", b"notes")
            .unwrap();
        let revision = daemon
            .get_metadata("lr:zotero:main")
            .unwrap()
            .unwrap()
            .metadata_revision;

        run_action(
            &daemon,
            &UiAction {
                action: "set_main_file".to_string(),
                return_to: "/?active=lr:zotero:main&tab=files".to_string(),
                item_id: Some("lr:zotero:main".to_string()),
                file_path: Some("notes.txt".to_string()),
                expected_revision: Some(revision),
                ..UiAction::default()
            },
        )
        .unwrap();

        let item = daemon.get_item("lr:zotero:main").unwrap().unwrap();
        assert_eq!(item.main_file.as_deref(), Some("notes.txt"));
        assert!(item.extra_files.iter().any(|path| path == "paper.pdf"));
    }

    #[test]
    fn available_categories_hide_common_categories() {
        let categories = vec![
            CategorySummary {
                path: "Archive".to_string(),
                item_ids: Vec::new(),
            },
            CategorySummary {
                path: "Inbox".to_string(),
                item_ids: Vec::new(),
            },
        ];
        let current = vec!["Inbox".to_string()];

        let paths = crate::state::available_categories(&categories, &current)
            .into_iter()
            .map(|category| category.path.clone())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["Archive".to_string()]);
    }
}
