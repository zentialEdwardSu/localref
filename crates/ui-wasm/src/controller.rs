//! Browser DOM controller for no-refresh Localref browsing interactions.

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::{Closure, JsValue};

use crate::api::fetch_state;
use crate::query::RouteState;
use crate::render::{render_detail_html, render_item_list_html};

/// Bind Localref browsing controls in the current browser document.
#[cfg(target_arch = "wasm32")]
pub fn start_controller() -> Result<(), JsValue> {
    bind_search()?;
    bind_category()?;
    bind_clicks()?;
    bind_popstate()?;
    Ok(())
}

fn document() -> Result<web_sys::Document, JsValue> {
    web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("missing document"))
}

fn bind_search() -> Result<(), JsValue> {
    let Some(element) = document()?.get_element_by_id("library-search") else {
        return Ok(());
    };
    let input: web_sys::HtmlInputElement = element.dyn_into()?;
    let bound_input = input.clone();
    let closure = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        let mut route = current_route();
        route.search = optional_text(&bound_input.value());
        route.selected_ids.clear();
        schedule_route(route);
    }) as Box<dyn FnMut(_)>);
    input.add_event_listener_with_callback(
        "change",
        closure.as_ref().unchecked_ref(),
    )?;
    closure.forget();
    Ok(())
}

fn bind_category() -> Result<(), JsValue> {
    let Some(element) = document()?.get_element_by_id("library-category")
    else {
        return Ok(());
    };
    let select: web_sys::HtmlSelectElement = element.dyn_into()?;
    let bound_select = select.clone();
    let closure = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        let mut route = current_route();
        route.category = optional_text(&bound_select.value());
        route.selected_ids.clear();
        schedule_route(route);
    }) as Box<dyn FnMut(_)>);
    select.add_event_listener_with_callback(
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
        if let Some(button) =
            closest(&element, "[data-route-active],[data-route-tab]")
        {
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
        }
    }) as Box<dyn FnMut(_)>);
    document()?.add_event_listener_with_callback(
        "click",
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
    if let Some(body) = doc.query_selector(".detail-body")? {
        body.set_inner_html(&render_detail_html(&state));
    }
    if push_history {
        let url = format!("/?{}", route.to_query_string());
        web_sys::window()
            .ok_or_else(|| JsValue::from_str("missing window"))?
            .history()?
            .push_state_with_url(&JsValue::NULL, "", Some(&url))?;
    }
    Ok(())
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
