//! Browser DOM controller for no-refresh Localref browsing interactions.

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::{Closure, JsValue};

use crate::api::fetch_state;
use crate::query::RouteState;
use crate::render::{
    render_detail_head_html, render_detail_html, render_events_html,
    render_item_list_html,
};

/// Bind Localref browsing controls in the current browser document.
#[cfg(target_arch = "wasm32")]
pub fn start_controller() -> Result<(), JsValue> {
    bind_filters()?;
    bind_clicks()?;
    bind_submits()?;
    bind_popstate()?;
    Ok(())
}

fn document() -> Result<web_sys::Document, JsValue> {
    web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("missing document"))
}

fn bind_filters() -> Result<(), JsValue> {
    let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let Some(target) = event.target() else {
            return;
        };
        let Ok(element) = target.dyn_into::<web_sys::Element>() else {
            return;
        };
        if element.id() != "library-search"
            && element.id() != "library-category"
        {
            return;
        }
        let mut route = current_route();
        if let Ok(doc) = document() {
            route.search = doc
                .get_element_by_id("library-search")
                .and_then(|element| {
                    element.dyn_into::<web_sys::HtmlInputElement>().ok()
                })
                .and_then(|input| optional_text(&input.value()));
            route.category = doc
                .get_element_by_id("library-category")
                .and_then(|element| {
                    element.dyn_into::<web_sys::HtmlSelectElement>().ok()
                })
                .and_then(|select| optional_text(&select.value()));
        }
        route.active_id = None;
        route.selected_ids.clear();
        schedule_route(route);
    }) as Box<dyn FnMut(_)>);
    document()?.add_event_listener_with_callback(
        "change",
        closure.as_ref().unchecked_ref(),
    )?;
    closure.forget();
    Ok(())
}

fn bind_clicks() -> Result<(), JsValue> {
    let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let Some(target) = event.target() else {
            return;
        };
        let Ok(element) = target.dyn_into::<web_sys::Element>() else {
            return;
        };
        if let Some(input) = closest(&element, ".row-check") {
            let Ok(input) = input.dyn_into::<web_sys::HtmlInputElement>()
            else {
                return;
            };
            let mut route = current_route();
            if input.checked() {
                if !route.selected_ids.contains(&input.value()) {
                    route.selected_ids.push(input.value());
                }
            } else {
                route.selected_ids.retain(|id| id != &input.value());
            }
            schedule_route(route);
            return;
        }
        if let Some(button) =
            closest(&element, "[data-route-active],[data-route-tab]")
        {
            if event.type_() == "dblclick" {
                schedule_open_file(button);
                return;
            }
            event.prevent_default();
            let mut route = current_route();
            if let Some(active) = button.get_attribute("data-route-active") {
                route.active_id = optional_text(&active);
                route.selected_ids.clear();
            }
            if let Some(tab) = button.get_attribute("data-route-tab") {
                if let Some(tab) = optional_text(&tab) {
                    route.tab = tab;
                }
            }
            schedule_route(route);
            return;
        }
        if let Some(toggle) = closest(&element, "[data-events-toggle]") {
            event.prevent_default();
            toggle_events(&toggle);
            return;
        }
        if let Some(button) = closest(&element, "[data-dismiss-dialog]") {
            event.prevent_default();
            dismiss_dialog(&button);
            return;
        }
    }) as Box<dyn FnMut(_)>);
    document()?.add_event_listener_with_callback(
        "click",
        closure.as_ref().unchecked_ref(),
    )?;
    document()?.add_event_listener_with_callback(
        "dblclick",
        closure.as_ref().unchecked_ref(),
    )?;
    closure.forget();
    Ok(())
}

fn bind_submits() -> Result<(), JsValue> {
    let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let Some(target) = event.target() else {
            return;
        };
        let Ok(element) = target.dyn_into::<web_sys::Element>() else {
            return;
        };
        let Some(form) = closest(&element, "form[data-route-action]") else {
            return;
        };
        event.prevent_default();
        let Ok(form) = form.dyn_into::<web_sys::HtmlFormElement>() else {
            return;
        };
        schedule_action_submit(form);
    }) as Box<dyn FnMut(_)>);
    document()?.add_event_listener_with_callback(
        "submit",
        closure.as_ref().unchecked_ref(),
    )?;
    closure.forget();
    Ok(())
}

fn bind_popstate() -> Result<(), JsValue> {
    let closure = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        schedule_route_without_push(current_route());
    }) as Box<dyn FnMut(_)>);
    web_sys::window()
        .ok_or_else(|| JsValue::from_str("missing window"))?
        .add_event_listener_with_callback(
            "popstate",
            closure.as_ref().unchecked_ref(),
        )?;
    closure.forget();
    Ok(())
}

fn closest(
    element: &web_sys::Element,
    selector: &str,
) -> Option<web_sys::Element> {
    element.closest(selector).ok().flatten()
}

fn current_route() -> RouteState {
    let query = web_sys::window()
        .and_then(|window| window.location().search().ok())
        .unwrap_or_default();
    let pairs = query
        .trim_start_matches('?')
        .split('&')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (decode_query(key), decode_query(value)))
        .collect::<Vec<_>>();
    RouteState::from_pairs(
        pairs.iter().map(|(key, value)| (key.as_str(), value.as_str())),
    )
}

fn schedule_route(route: RouteState) {
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = visit_route(route, true).await {
            web_sys::console::error_1(&error);
        }
    });
}

fn schedule_route_without_push(route: RouteState) {
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = visit_route(route, false).await {
            web_sys::console::error_1(&error);
        }
    });
}

async fn visit_route(
    route: RouteState,
    push_history: bool,
) -> Result<(), JsValue> {
    let state = fetch_state(&route).await?;
    let doc = document()?;
    if let Some(list) = doc.get_element_by_id("library-list") {
        list.set_inner_html(&render_item_list_html(&state));
    }
    if let Some(head) = doc.query_selector("[data-primary-detail-head]")? {
        head.set_inner_html(&render_detail_head_html(&state));
    }
    if let Some(body) = doc.query_selector(".detail-body")? {
        body.set_inner_html(&render_detail_html(&state));
    }
    if let Some(panel) = doc.query_selector(".event-panel")? {
        panel.set_inner_html(&render_events_html(&state));
    }
    sync_detail_visibility(&doc, state.tab == "events")?;
    if push_history {
        let url = format!("/?{}", route.to_query_string());
        web_sys::window()
            .ok_or_else(|| JsValue::from_str("missing window"))?
            .history()?
            .push_state_with_url(&JsValue::NULL, "", Some(&url))?;
    }
    Ok(())
}

/// Keep the primary detail and events panel consistent after client routing.
fn sync_detail_visibility(
    doc: &web_sys::Document,
    events_open: bool,
) -> Result<(), JsValue> {
    if let Some(primary_detail) =
        doc.query_selector("[data-primary-detail]")?
    {
        if let Some(element) = primary_detail.dyn_ref::<web_sys::HtmlElement>()
        {
            element.set_hidden(events_open);
        }
    }
    if let Some(event_panel) = doc.query_selector(".event-panel")? {
        if let Some(element) = event_panel.dyn_ref::<web_sys::HtmlElement>() {
            element.set_hidden(!events_open);
        }
    }
    if let Some(toggle) = doc.query_selector("[data-events-toggle]")? {
        toggle.set_attribute(
            "aria-pressed",
            if events_open { "true" } else { "false" },
        )?;
        let class_list = toggle.class_list();
        if events_open {
            class_list.add_1("is-active")?;
        } else {
            class_list.remove_1("is-active")?;
        }
    }
    Ok(())
}

fn toggle_events(toggle: &web_sys::Element) {
    let Ok(doc) = document() else {
        return;
    };
    let Ok(primary_detail) = doc.query_selector("[data-primary-detail]")
    else {
        return;
    };
    let Ok(event_panel) = doc.query_selector(".event-panel") else {
        return;
    };
    let Some(event_panel) = event_panel else {
        return;
    };
    let open = event_panel
        .dyn_ref::<web_sys::HtmlElement>()
        .map(web_sys::HtmlElement::hidden)
        .unwrap_or(false);
    if let Some(primary_detail) = primary_detail {
        if let Some(element) = primary_detail.dyn_ref::<web_sys::HtmlElement>()
        {
            element.set_hidden(open);
        }
    }
    if let Some(element) = event_panel.dyn_ref::<web_sys::HtmlElement>() {
        element.set_hidden(!open);
    }
    let _ = toggle
        .set_attribute("aria-pressed", if open { "true" } else { "false" });
    let class_list = toggle.class_list();
    if open {
        let _ = class_list.add_1("is-active");
    } else {
        let _ = class_list.remove_1("is-active");
    }
}

fn dismiss_dialog(button: &web_sys::Element) {
    if let Ok(Some(dialog)) = button.closest(".rules-result-dialog") {
        dialog.remove();
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(location) = window.location().href() else {
        return;
    };
    let Ok(url) = web_sys::Url::new(&location) else {
        return;
    };
    url.search_params().delete("rules_status");
    url.search_params().delete("rules_error");
    let _ = window.history().and_then(|history| {
        history.replace_state_with_url(&JsValue::NULL, "", Some(&url.href()))
    });
}

fn schedule_open_file(button: web_sys::Element) {
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = open_file(button).await {
            web_sys::console::error_1(&error);
        }
    });
}

async fn open_file(button: web_sys::Element) -> Result<(), JsValue> {
    let file_path = button.get_attribute("data-open-file").unwrap_or_default();
    let item_id =
        button.get_attribute("data-route-active").unwrap_or_default();
    if file_path.trim().is_empty() || item_id.trim().is_empty() {
        return Ok(());
    }
    let params = web_sys::UrlSearchParams::new()?;
    params.set("return_to", &current_return_to());
    params.set("action", "open_file");
    params.set("item_id", &item_id);
    params.set("file_path", &file_path);
    post_form_encoded("/ui/action", &params).await.map(|_| ())
}

fn schedule_action_submit(form: web_sys::HtmlFormElement) {
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = submit_action(form).await {
            web_sys::console::error_1(&error);
        }
    });
}

async fn submit_action(form: web_sys::HtmlFormElement) -> Result<(), JsValue> {
    let action_url = form
        .get_attribute("action")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| form.action());
    let keep_category_editor_open = form
        .closest(".category-editor")?
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        .map(|element| !element.hidden())
        .unwrap_or(false);
    let form_data = web_sys::FormData::new_with_form(&form)?;
    let params = web_sys::UrlSearchParams::new_with_str_sequence_sequence(
        &form_data.into(),
    )?;
    let response = post_form_encoded(&action_url, &params).await?;
    replace_shell(response).await?;
    if keep_category_editor_open {
        if let Some(editor) = document()?.query_selector(".category-editor")? {
            let _ = editor.set_attribute("open", "");
        }
    }
    Ok(())
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
        web_sys::window()
            .ok_or_else(|| JsValue::from_str("missing window"))?
            .fetch_with_request(&request),
    )
    .await?;
    response.dyn_into::<web_sys::Response>()
}

async fn replace_shell(response: web_sys::Response) -> Result<(), JsValue> {
    if !response.ok() {
        return Err(JsValue::from_str(&format!(
            "action failed: {}",
            response.status()
        )));
    }
    let response_url = response.url();
    let text = wasm_bindgen_futures::JsFuture::from(response.text()?).await?;
    let Some(text) = text.as_string() else {
        return Err(JsValue::from_str("action body is not text"));
    };
    let parser = web_sys::DomParser::new()?;
    let doc =
        parser.parse_from_string(&text, web_sys::SupportedType::TextHtml)?;
    let Some(next_shell) = doc.query_selector(".app-shell")? else {
        return Err(JsValue::from_str("action response missing app shell"));
    };
    let Some(current_shell) = document()?.query_selector(".app-shell")? else {
        return Err(JsValue::from_str("current document missing app shell"));
    };
    current_shell.set_inner_html(&next_shell.inner_html());
    web_sys::window()
        .ok_or_else(|| JsValue::from_str("missing window"))?
        .history()?
        .replace_state_with_url(&JsValue::NULL, "", Some(&response_url))?;
    Ok(())
}

fn current_return_to() -> String {
    let Some(window) = web_sys::window() else {
        return "/".to_string();
    };
    let location = window.location();
    let path = location.pathname().unwrap_or_else(|_| "/".to_string());
    let search = location.search().unwrap_or_default();
    format!("{path}{search}")
}

fn optional_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

fn decode_query(value: &str) -> String {
    let mut bytes = Vec::new();
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let Some(high) = input.next().and_then(hex_value) else {
                    continue;
                };
                let Some(low) = input.next().and_then(hex_value) else {
                    continue;
                };
                bytes.push((high << 4) | low);
            }
            byte => bytes.push(byte),
        }
    }
    String::from_utf8(bytes).unwrap_or_default()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
