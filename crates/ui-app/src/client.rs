//! Browser-side hydration and HTTP helpers for the Localref Leptos app.

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::JsValue;

use crate::app;
use crate::model::UiState;
use crate::route::{RouteState, state_url};

/// Hydrate the server-rendered Localref document body.
pub fn hydrate() -> Result<(), JsValue> {
    let state = initial_state()?;
    leptos::mount::hydrate_body(move || app::body_app(state));
    Ok(())
}

/// Visit one UI route without replacing the whole page.
pub fn visit_route(
    route: RouteState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
    push_history: bool,
) {
    wasm_bindgen_futures::spawn_local(async move {
        match fetch_state(&route).await {
            Ok(state) => {
                let events_open = state.tab == "events";
                set_state.set(state);
                set_events_open.set(events_open);
                if push_history {
                    if let Err(error) = push_route(&route) {
                        web_sys::console::error_1(&error);
                    }
                }
            }
            Err(error) => web_sys::console::error_1(&error),
        }
    });
}

/// Submit a form action and hydrate state from the redirected HTML response.
pub fn submit_action(
    event: leptos::ev::SubmitEvent,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) {
    let Some(target) = event.target() else {
        return;
    };
    let Ok(form) = target.dyn_into::<web_sys::HtmlFormElement>() else {
        return;
    };
    wasm_bindgen_futures::spawn_local(async move {
        match submit_form(form).await {
            Ok((state, url)) => {
                let events_open = state.tab == "events";
                set_state.set(state);
                set_events_open.set(events_open);
                if let Err(error) = replace_url(&url) {
                    web_sys::console::error_1(&error);
                }
            }
            Err(error) => web_sys::console::error_1(&error),
        }
    });
}

/// Submit the parent form for a changed input and hydrate the response.
pub fn submit_changed_form(
    event: leptos::ev::Event,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) {
    let Some(target) = event.target() else {
        return;
    };
    let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() else {
        return;
    };
    let Some(form) = input.form() else {
        return;
    };
    wasm_bindgen_futures::spawn_local(async move {
        match submit_form(form).await {
            Ok((state, url)) => {
                let events_open = state.tab == "events";
                set_state.set(state);
                set_events_open.set(events_open);
                if let Err(error) = replace_url(&url) {
                    web_sys::console::error_1(&error);
                }
            }
            Err(error) => web_sys::console::error_1(&error),
        }
    });
}

/// Upload files selected from the file input and refresh hydrated state.
pub fn upload_input_files(
    event: leptos::ev::Event,
    item_id: String,
    return_to: String,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) {
    let Some(target) = event.target() else {
        return;
    };
    let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() else {
        return;
    };
    let Some(files) = input.files() else {
        return;
    };
    input.set_value("");
    upload_file_list(files, item_id, return_to, set_state, set_events_open);
}

/// Upload files dropped on the files pane and refresh hydrated state.
pub fn upload_dropped_files(
    event: leptos::ev::DragEvent,
    item_id: String,
    return_to: String,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) {
    let Some(files) = event.data_transfer().and_then(|data| data.files())
    else {
        return;
    };
    upload_file_list(files, item_id, return_to, set_state, set_events_open);
}

/// Remove rules-result query keys after the notice is dismissed.
pub fn clear_rules_notice_query() {
    if let Err(error) = clear_rules_notice_query_inner() {
        web_sys::console::error_1(&error);
    }
}

fn initial_state() -> Result<UiState, JsValue> {
    let document = document()?;
    let element = document
        .get_element_by_id("localref-ui-state")
        .ok_or_else(|| JsValue::from_str("missing Localref UI state"))?;
    let text = element
        .text_content()
        .ok_or_else(|| JsValue::from_str("Localref UI state is empty"))?;
    serde_json::from_str(&text).map_err(|error| {
        JsValue::from_str(&format!(
            "failed to decode Localref UI state: {error}"
        ))
    })
}

async fn fetch_state(route: &RouteState) -> Result<UiState, JsValue> {
    let response_value = wasm_bindgen_futures::JsFuture::from(
        window()?.fetch_with_str(&state_url(route)),
    )
    .await?;
    let response: web_sys::Response = response_value.dyn_into()?;
    if !response.ok() {
        return Err(JsValue::from_str(&format!(
            "state fetch failed: {}",
            response.status()
        )));
    }
    let text_value =
        wasm_bindgen_futures::JsFuture::from(response.text()?).await?;
    let text = text_value
        .as_string()
        .ok_or_else(|| JsValue::from_str("state body is not text"))?;
    serde_json::from_str(&text).map_err(|error| {
        JsValue::from_str(&format!("state decode failed: {error}"))
    })
}

async fn submit_form(
    form: web_sys::HtmlFormElement,
) -> Result<(UiState, String), JsValue> {
    let action_url = form
        .get_attribute("action")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| form.action());
    let form_data = web_sys::FormData::new_with_form(&form)?;
    let params = web_sys::UrlSearchParams::new_with_str_sequence_sequence(
        &form_data.into(),
    )?;
    let response = post_form_encoded(&action_url, &params).await?;
    if !response.ok() {
        return Err(JsValue::from_str(&format!(
            "action failed: {}",
            response.status()
        )));
    }
    let response_url = response.url();
    let text = wasm_bindgen_futures::JsFuture::from(response.text()?).await?;
    let text = text
        .as_string()
        .ok_or_else(|| JsValue::from_str("action body is not text"))?;
    let state = state_from_html(&text)?;
    Ok((state, response_url))
}

fn upload_file_list(
    files: web_sys::FileList,
    item_id: String,
    return_to: String,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) {
    if files.length() == 0 || item_id.trim().is_empty() {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        match upload_files(files, item_id, return_to).await {
            Ok((state, url)) => {
                let events_open = state.tab == "events";
                set_state.set(state);
                set_events_open.set(events_open);
                if let Err(error) = replace_url(&url) {
                    web_sys::console::error_1(&error);
                }
            }
            Err(error) => web_sys::console::error_1(&error),
        }
    });
}

async fn upload_files(
    files: web_sys::FileList,
    item_id: String,
    return_to: String,
) -> Result<(UiState, String), JsValue> {
    let form_data = web_sys::FormData::new()?;
    form_data.append_with_str("item_id", &item_id)?;
    form_data.append_with_str("return_to", &return_to)?;
    for index in 0..files.length() {
        let Some(file) = files.item(index) else {
            continue;
        };
        let filename = file.name();
        let blob: &web_sys::Blob = file.unchecked_ref();
        form_data.append_with_blob_and_filename("file", blob, &filename)?;
    }
    let response = post_multipart("/ui/upload", &form_data).await?;
    if !response.ok() {
        return Err(JsValue::from_str(&format!(
            "upload failed: {}",
            response.status()
        )));
    }
    let response_url = response.url();
    let text = wasm_bindgen_futures::JsFuture::from(response.text()?).await?;
    let text = text
        .as_string()
        .ok_or_else(|| JsValue::from_str("upload body is not text"))?;
    let state = state_from_html(&text)?;
    Ok((state, response_url))
}

async fn post_multipart(
    url: &str,
    form_data: &web_sys::FormData,
) -> Result<web_sys::Response, JsValue> {
    let headers = web_sys::Headers::new()?;
    headers.set("X-Localref-UI-Router", "1")?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_mode(web_sys::RequestMode::SameOrigin);
    init.set_headers(&headers);
    init.set_body(form_data.as_ref());
    let request = web_sys::Request::new_with_str_and_init(url, &init)?;
    let response = wasm_bindgen_futures::JsFuture::from(
        window()?.fetch_with_request(&request),
    )
    .await?;
    response.dyn_into::<web_sys::Response>()
}

async fn post_form_encoded(
    url: &str,
    params: &web_sys::UrlSearchParams,
) -> Result<web_sys::Response, JsValue> {
    let headers = web_sys::Headers::new()?;
    headers.set("content-type", "application/x-www-form-urlencoded")?;
    headers.set("X-Localref-UI-Router", "1")?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_mode(web_sys::RequestMode::SameOrigin);
    init.set_headers(&headers);
    init.set_body(&params.to_string().into());
    let request = web_sys::Request::new_with_str_and_init(url, &init)?;
    let response = wasm_bindgen_futures::JsFuture::from(
        window()?.fetch_with_request(&request),
    )
    .await?;
    response.dyn_into::<web_sys::Response>()
}

fn state_from_html(html: &str) -> Result<UiState, JsValue> {
    let parser = web_sys::DomParser::new()?;
    let doc =
        parser.parse_from_string(html, web_sys::SupportedType::TextHtml)?;
    let element =
        doc.get_element_by_id("localref-ui-state").ok_or_else(|| {
            JsValue::from_str("action response missing UI state")
        })?;
    let text = element.text_content().ok_or_else(|| {
        JsValue::from_str("action response UI state is empty")
    })?;
    serde_json::from_str(&text).map_err(|error| {
        JsValue::from_str(&format!("action state decode failed: {error}"))
    })
}

fn push_route(route: &RouteState) -> Result<(), JsValue> {
    window()?.history()?.push_state_with_url(
        &JsValue::NULL,
        "",
        Some(&route.to_path()),
    )
}

fn replace_url(url: &str) -> Result<(), JsValue> {
    window()?.history()?.replace_state_with_url(&JsValue::NULL, "", Some(url))
}

fn clear_rules_notice_query_inner() -> Result<(), JsValue> {
    let location = window()?.location().href()?;
    let url = web_sys::Url::new(&location)?;
    url.search_params().delete("rules_status");
    url.search_params().delete("rules_error");
    replace_url(&url.href())
}

fn document() -> Result<web_sys::Document, JsValue> {
    window()?.document().ok_or_else(|| JsValue::from_str("missing document"))
}

fn window() -> Result<web_sys::Window, JsValue> {
    web_sys::window().ok_or_else(|| JsValue::from_str("missing window"))
}
