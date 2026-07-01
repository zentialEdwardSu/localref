//! Detail pane: route-driven tab bar and content dispatch.

use leptos::prelude::*;

use crate::model::UiState;
use crate::route::RouteState;

/// Render the detail pane header tabs (route-driven, not signal-driven).
#[allow(clippy::single_call_fn)]
pub fn render_detail_tabs(
    state: ReadSignal<UiState>,
    set_state: WriteSignal<UiState>,
) -> impl IntoView {
    view! {
        <nav class="flex items-center gap-1 border-b border-border px-4">
            {move || {
                let s = state.with(|s| s.clone());
                if s.selected_ids.is_empty() {
                    view! {
                        {tab_button("Metadata".to_string(), "metadata".to_string(), &s, set_state)}
                        {tab_button("Files".to_string(), "files".to_string(), &s, set_state)}
                    }.into_any()
                } else {
                    ().into_any()
                }
            }}
            {move || {
                let s = state.with(|s| s.clone());
                tab_button("Rules".to_string(), "rules".to_string(), &s, set_state)
            }}
            {move || {
                let tabs: Vec<_> = state.with(|s| {
                    s.plugin_tabs.iter().map(|t| (t.label.clone(), t.tab_key.clone())).collect()
                });
                let s = state.with(|s| s.clone());
                tabs.into_iter().map(|(label, key)| {
                    tab_button(label, key, &s, set_state)
                }).collect::<Vec<_>>()
            }}
        </nav>
    }
}

/// One route-driven tab button (styled like rust-ui TabsTrigger but wired to visit_route).
fn tab_button(
    label: String,
    tab: String,
    state: &UiState,
    set_state: WriteSignal<UiState>,
) -> impl IntoView + use<> {
    let route_state = state.clone();
    let is_active = state.tab == tab;
    let tab_clone = tab.clone();

    view! {
        <button
            type="button"
            class=if is_active {
                "inline-flex items-center justify-center px-3 py-2 text-sm font-medium border-b-2 border-primary text-foreground"
            } else {
                "inline-flex items-center justify-center px-3 py-2 text-sm font-medium border-b-2 border-transparent text-muted-foreground hover:text-foreground"
            }
            on:click=move |event| {
                event.prevent_default();
                let mut route = RouteState::from_ui_state(&route_state);
                route.tab.clone_from(&tab_clone);
                super::visit_route(route, set_state, true);
            }
        >
            {label}
        </button>
    }
}

/// Render the active detail body.
#[allow(clippy::single_call_fn)]
pub fn render_detail_body(
    state: ReadSignal<UiState>,
    set_state: WriteSignal<UiState>,
) -> impl IntoView {
    move || state.with(|s| {
        if !s.selected_ids.is_empty()
            && s.tab != "rules"
            && !s.tab.starts_with("plugin:")
        {
            return super::metadata::render_metadata(s, set_state);
        }
        if s.tab.starts_with("plugin:")
            && let Some(page) = s.plugin_active_page.clone()
        {
            let return_to = s.return_to.clone();
            let selected_csv = s.selected_ids.join(",");
            let active_value = s.active_id.clone().unwrap_or_default();
            let error_view = s.plugin_error.clone().map(|msg| view! {
                <div class="plugin-error" role="alert"><h3>"Plugin error"</h3><p>{msg}</p></div>
            });
            return view! {
                <div class="p-4">
                    {error_view}
                    {super::plugins::render_plugin_page(page, return_to, selected_csv, active_value)}
                </div>
            }.into_any();
        }
        match s.tab.as_str() {
            "files" => super::files::render_files(s, set_state).into_any(),
            "rules" => super::rules::render_rules(s, set_state).into_any(),
            _ => super::metadata::render_metadata(s, set_state),
        }
    })
}

/// Return the right pane title.
pub fn detail_title(state: &UiState) -> String {
    if state.tab == "rules" {
        return "Rules Editor".to_string();
    }
    match state.selected_ids.len() {
        0 => state
            .active_detail
            .as_ref()
            .map(|item| item.title.clone())
            .or_else(|| {
                state.active_id.as_ref().and_then(|id| {
                    state.items.iter().find(|item| &item.id == id)
                }).map(|item| item.title.clone())
            })
            .unwrap_or_else(|| "No item selected".to_string()),
        1 => "Selected 1 item".to_string(),
        count => format!("Selected {count} items"),
    }
}
