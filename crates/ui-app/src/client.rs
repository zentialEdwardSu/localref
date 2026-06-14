//! Browser-side hydration and HTTP helpers for the Localref Leptos app.

use std::cell::Cell;
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::{Closure, JsValue};

use crate::app;
use crate::model::UiState;
use crate::route::{RouteState, state_url};

/// Hydrate the server-rendered Localref document body.
pub fn hydrate() -> Result<(), JsValue> {
    let state = initial_state()?;
    leptos::mount::hydrate_body(move || app::body_app(state));
    attach_plugin_form_listeners()?;
    Ok(())
}

/// Wire Tier-1 display refresh and Tier-2 preview listeners on all plugin forms.
///
/// Called once after hydrate_body. Each `form.plugin-form` gets an `input`
/// listener that refreshes display readouts and debounces preview fetches.
// Called only from `hydrate`; kept separate to isolate the listener wiring.
#[allow(clippy::single_call_fn)]
fn attach_plugin_form_listeners() -> Result<(), JsValue> {
    let document = document()?;
    let forms = document.query_selector_all("form.plugin-form")?;
    for i in 0..forms.length() {
        let Some(node) = forms.item(i) else { continue };
        let Ok(form_el) = node.dyn_into::<web_sys::HtmlFormElement>() else {
            continue;
        };
        // Initial Tier-1 render.
        refresh_plugin_displays(form_el.as_ref());

        let preview_attr = form_el.get_attribute("data-plugin-preview");
        let debounce_ms = preview_attr
            .as_deref()
            .and_then(|attr| attr.split(':').nth(1))
            .and_then(|ms| ms.parse::<i32>().ok())
            .unwrap_or(300);
        let has_preview = preview_attr
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());

        // Shared timer handle: -1 means no pending timer.
        let timer_handle: Rc<Cell<i32>> = Rc::new(Cell::new(-1));

        let form_for_listener = form_el.clone();
        let timer_for_listener = timer_handle.clone();

        let listener: Closure<dyn Fn(web_sys::Event)> =
            Closure::new(move |_event: web_sys::Event| {
                refresh_plugin_displays(form_for_listener.as_ref());

                if !has_preview {
                    return;
                }

                // Clear any pending debounce timer.
                let old_handle = timer_for_listener.get();
                if old_handle >= 0 {
                    if let Ok(win) = window() {
                        win.clear_timeout_with_handle(old_handle);
                    }
                }

                // Schedule a new debounced preview fetch.
                let form_for_timeout = form_for_listener.clone();
                let timeout_cb: Closure<dyn Fn()> =
                    Closure::new(move || run_plugin_preview(form_for_timeout.clone()));
                let Ok(win) = window() else { return };
                match win
                    .set_timeout_with_callback_and_timeout_and_arguments_0(
                        timeout_cb.as_ref().unchecked_ref(),
                        debounce_ms,
                    ) {
                    Ok(handle) => timer_for_listener.set(handle),
                    Err(error) => web_sys::console::error_1(&error),
                }
                // The timeout fires once; forget the closure to avoid dropping it early.
                timeout_cb.forget();
            });

        form_el
            .add_event_listener_with_callback(
                "input",
                listener.as_ref().unchecked_ref(),
            )?;
        // Leak the listener — it lives for the page lifetime.
        listener.forget();
    }
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

/// Recompute every `data-display` readout in a plugin form from its template.
///
/// Supported tokens: `{selection.count}` from the hidden `selected` input
/// (comma-separated list) and `{field.<name>}` from each field's current value.
fn refresh_plugin_displays(form: &web_sys::Element) {
    let selected_count = form
        .query_selector("input[name=selected]")
        .ok()
        .flatten()
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.value())
        .map_or(0, |v| v.split(',').filter(|s| !s.is_empty()).count());

    let Ok(displays) = form.query_selector_all("[data-display]") else {
        return;
    };
    for i in 0..displays.length() {
        let Some(node) = displays.item(i) else { continue };
        let Some(el) = node.dyn_ref::<web_sys::Element>() else { continue };
        let Some(template) = el.get_attribute("data-template") else {
            continue;
        };
        let mut text =
            template.replace("{selection.count}", &selected_count.to_string());
        if let Ok(fields) = form.query_selector_all("[data-field]") {
            for j in 0..fields.length() {
                let Some(fnode) = fields.item(j) else { continue };
                let Some(fel) = fnode.dyn_ref::<web_sys::Element>() else {
                    continue;
                };
                let Some(name) = fel.get_attribute("data-field") else {
                    continue;
                };
                let value = input_value_within(fel).unwrap_or_default();
                text = text.replace(&format!("{{field.{name}}}"), &value);
            }
        }
        el.set_text_content(Some(&text));
    }
}

/// Read the current value of the input/select/textarea inside a field wrapper.
// Called only from `refresh_plugin_displays`; split out for readability.
#[allow(clippy::single_call_fn)]
fn input_value_within(field: &web_sys::Element) -> Option<String> {
    let control = field.query_selector(".plugin-field-input").ok().flatten()?;
    if let Some(input) = control.dyn_ref::<web_sys::HtmlInputElement>() {
        return Some(input.value());
    }
    if let Some(select) = control.dyn_ref::<web_sys::HtmlSelectElement>() {
        return Some(select.value());
    }
    if let Some(area) = control.dyn_ref::<web_sys::HtmlTextAreaElement>() {
        return Some(area.value());
    }
    None
}

/// Fire a Tier-2 preview for a plugin form and drop the text into its pane.
///
/// Reads `data-plugin-preview` (`"<action>:<debounce_ms>:<into>"`). The
/// debounce is handled by the caller; this performs one fetch.
// Called only from the debounced timeout closure in `attach_plugin_form_listeners`.
#[allow(clippy::single_call_fn)]
fn run_plugin_preview(form: web_sys::HtmlFormElement) {
    let Some(attr) = form.get_attribute("data-plugin-preview") else {
        return;
    };
    let mut parts = attr.split(':');
    let (Some(action), Some(_ms), Some(into)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return;
    };
    let plugin = form.get_attribute("data-plugin").unwrap_or_default();
    let action = action.to_string();
    let into = into.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        let Ok(params) = form_encoded_with_action(&form, &action) else {
            return;
        };
        let url = format!("/plugin/{plugin}/preview");
        match post_form_encoded_text(&url, &params).await {
            Ok(text) => set_preview_pane(form.as_ref(), &into, &text),
            Err(error) => web_sys::console::error_1(&error),
        }
    });
}

/// Drop preview JSON (`{"text":"..."}`) into the named `data-display` pane.
// Called only from `run_plugin_preview`; split out for readability.
#[allow(clippy::single_call_fn)]
fn set_preview_pane(form: &web_sys::Element, into: &str, json: &str) {
    let text = serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| {
            v.get("text").and_then(|t| t.as_str()).map(str::to_string)
        })
        .unwrap_or_default();
    if let Ok(Some(node)) =
        form.query_selector(&format!("[data-display=\"{into}\"]"))
    {
        node.set_text_content(Some(&text));
    }
}

/// Serialize a form's named inputs as `application/x-www-form-urlencoded`,
/// appending `plugin_action=<action>`.
// Called only from `run_plugin_preview`; split out for readability.
#[allow(clippy::single_call_fn)]
fn form_encoded_with_action(
    form: &web_sys::HtmlFormElement,
    action: &str,
) -> Result<web_sys::UrlSearchParams, JsValue> {
    let form_data = web_sys::FormData::new_with_form(form)?;
    let params =
        web_sys::UrlSearchParams::new_with_str_sequence_sequence(&form_data.into())?;
    params.append("plugin_action", action);
    Ok(params)
}

/// POST `application/x-www-form-urlencoded` and return the raw response text.
// Called only from `run_plugin_preview`; split out for readability.
#[allow(clippy::single_call_fn)]
async fn post_form_encoded_text(
    url: &str,
    params: &web_sys::UrlSearchParams,
) -> Result<String, JsValue> {
    let response = post_form_encoded(url, params).await?;
    if !response.ok() {
        return Err(JsValue::from_str(&format!(
            "preview failed: {}",
            response.status()
        )));
    }
    let text_value =
        wasm_bindgen_futures::JsFuture::from(response.text()?).await?;
    text_value
        .as_string()
        .ok_or_else(|| JsValue::from_str("preview body is not text"))
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
