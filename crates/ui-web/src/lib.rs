#![recursion_limit = "256"]

//! Server-rendered Localref web UI.
//!
//! The UI renders through Leptos on top of Axum and talks directly to the
//! daemon facade. Browser forms post back to this router, then redirect to a
//! URL-query state that can be bookmarked or opened from the tray.

mod actions;
mod assets;
mod components;
mod dto;
mod state;

use actions::{UiAction, run_action};
use assets::{favicon, ui_css, ui_wasm, ui_wasm_js};
use axum::Json;
use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use components::render_page;
use dto::UiStateDto;
use localref_core::LocalrefDaemon;
use state::{UiModel, UiQuery, escape_text, return_path};

/// Build the server-rendered UI router for one daemon facade.
pub fn router_with_daemon(daemon: LocalrefDaemon) -> Router {
    Router::new()
        .route("/", get(home))
        .route("/ui/state", get(ui_state))
        .route("/assets/favicon.ico", get(favicon))
        .route("/assets/localref-ui.css", get(ui_css))
        .route("/assets/localref-ui.js", get(ui_wasm_js))
        .route("/assets/localref-ui_bg.wasm", get(ui_wasm))
        .route("/ui/action", post(action))
        .with_state(UiState { daemon })
}

#[derive(Clone)]
struct UiState {
    daemon: LocalrefDaemon,
}

async fn home(
    State(state): State<UiState>,
    Query(query): Query<UiQuery>,
) -> Response {
    match UiModel::load(&state.daemon, query) {
        Ok(model) => Html(render_page(model)).into_response(),
        Err(error) => Html(format!(
            "<!doctype html><title>Localref</title><main><h1>Localref</h1><p>{}</p></main>",
            escape_text(&error.to_string())
        ))
        .into_response(),
    }
}

async fn ui_state(
    State(state): State<UiState>,
    Query(query): Query<UiQuery>,
) -> Response {
    match UiModel::load(&state.daemon, query) {
        Ok(model) => Json(UiStateDto::from_model(model)).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            .into_response(),
    }
}

async fn action(
    State(state): State<UiState>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::Request;
    use localref_core::storage::CategorySummary;
    use localref_core::types::CategoryPath;
    use tower::ServiceExt;

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
        assert!(html.contains("filterRouteFrom"));
        assert!(!html.contains("document.querySelectorAll('.library-row')"));
        assert!(!html.contains("cdn.tailwindcss.com"));
        assert!(!html.contains("Update Selection"));

        let events_index = html.find(">Events</button>").unwrap();
        let metadata_index = html.find(">Metadata</button>").unwrap();
        let files_index = html.find(">Files</button>").unwrap();
        let editor_index = html.find("Current Categories:").unwrap();
        let create_index = html.find("Create Category").unwrap();
        assert!(events_index < metadata_index);
        assert!(metadata_index < files_index);
        assert!(editor_index < create_index);
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
        assert!(script.contains("localref-ui_bg.wasm"));
        assert!(!script.contains("filterRouteFrom"));

        let wasm_response = app
            .oneshot(
                Request::builder()
                    .uri("/assets/localref-ui_bg.wasm")
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
        assert!(html.contains("history.pushState"));
        assert!(html.contains("fetch(routeUrl"));
        assert!(html.contains("DOMParser"));
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
        assert!(html.contains("addEventListener('submit'"));
        assert!(html.contains("form.getAttribute('action') || form.action"));
        assert!(html.contains("fetch(actionUrl"));
        assert!(html.contains("keepCategoryEditorOpen"));
        assert!(html.contains("setAttribute('open', '')"));
        assert!(html.contains("new URLSearchParams(new FormData(form))"));
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
        assert!(html.contains("form.selection-form"));
        assert!(html.contains("new URLSearchParams(new FormData(form))"));
        assert!(html.contains("formData.getAll('item')"));
        assert!(html.contains("params.set('selected', selected.join(','))"));
        assert!(html.contains("scheduleFilterRoute"));
        assert!(html.contains("routeUrl.searchParams.delete('selected')"));
        assert!(html.contains(".row-check"));
        assert!(html.contains("visitRoute(selectionRouteFrom(form), true)"));
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
                    .uri("/ui/state?q=alpha&tab=files")
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
        assert!(html.contains("addEventListener('dblclick'"));
        assert!(html.contains("openItemFile(button)"));
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
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let app = router_with_daemon(daemon);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/?tab=files")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Open Folder"));
        assert!(html.contains("Add File"));
        assert!(html.contains("Import File"));
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

        let paths = state::available_categories(&categories, &current)
            .into_iter()
            .map(|category| category.path.clone())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["Archive".to_string()]);
    }
}
