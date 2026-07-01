//! Compact toolbar: brand, search, category, actions, view toggles, theme.

use leptos::prelude::*;

use crate::model::UiState;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonVariant, ButtonSize};
use crate::components::ui::switch::Switch;
use crate::components::ui::theme_toggle::ThemeToggle;

/// Render the compact toolbar.
#[allow(clippy::single_call_fn, clippy::too_many_arguments)]
pub fn render_topbar(
    state: ReadSignal<UiState>,
    set_state: WriteSignal<UiState>,
    detail_open: ReadSignal<bool>,
    set_detail_open: WriteSignal<bool>,
    rules_open: ReadSignal<bool>,
    set_rules_open: WriteSignal<bool>,
    plugins_open: ReadSignal<bool>,
    set_plugins_open: WriteSignal<bool>,
) -> impl IntoView {
    view! {
        <header class="shrink-0 z-20 border-b border-border bg-card px-4 py-1.5 flex items-center gap-2 flex-wrap">
            // Brand
            <span class="text-sm font-semibold text-foreground whitespace-nowrap">
                {move || state.with(|s| s.repo_name.clone())}
            </span>
            <Badge variant=BadgeVariant::Secondary size=crate::components::ui::badge::BadgeSize::Sm>
                {move || state.with(|s| s.status_label.clone())}
            </Badge>

            // Spacer
            <div class="flex-1" />

            // View dropdown
            <div class="relative">
                <button
                    type="button"
                    id="view-dropdown-trigger"
                    class="h-7 inline-flex items-center gap-1 border border-input bg-transparent px-2 text-xs cursor-pointer hover:bg-accent hover:text-accent-foreground"
                    on:click=move |event| {
                        event.stop_propagation();
                        // Toggle dropdown via JS attr
                        #[cfg(feature = "hydrate")]
                        {
                            use wasm_bindgen::JsCast;
                            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                                if let Some(menu) = doc.get_element_by_id("view-dropdown-menu") {
                                    let hidden = menu.get_attribute("data-open").is_none();
                                    if hidden {
                                        menu.set_attribute("data-open", "").ok();
                                    } else {
                                        menu.remove_attribute("data-open").ok();
                                    }
                                }
                            }
                        }
                    }
                >
                    "View"
                    <span class="text-[10px] text-muted-foreground">"▾"</span>
                </button>
                <div
                    id="view-dropdown-menu"
                    class="absolute top-full left-0 mt-1 z-50 min-w-[160px] border border-border bg-card shadow-md py-1 hidden data-[open]:block"
                >
                    <button
                        type="button"
                        class="w-full text-left px-3 py-1.5 text-xs hover:bg-accent hover:text-accent-foreground flex items-center gap-2"
                        on:click=move |_| {
                            set_detail_open.update(|v| *v = !*v);
                            close_view_menu();
                        }
                    >
                        <span class="w-3 text-[10px]">{move || if detail_open.get() { "✓" } else { "" }}</span>
                        "Detail Panel"
                    </button>
                    <button
                        type="button"
                        class="w-full text-left px-3 py-1.5 text-xs hover:bg-accent hover:text-accent-foreground flex items-center gap-2"
                        on:click=move |_| {
                            set_rules_open.update(|v| *v = !*v);
                            close_view_menu();
                        }
                    >
                        <span class="w-3 text-[10px]">{move || if rules_open.get() { "✓" } else { "" }}</span>
                        "Rules Editor"
                    </button>
                    <button
                        type="button"
                        class="w-full text-left px-3 py-1.5 text-xs hover:bg-accent hover:text-accent-foreground flex items-center gap-2"
                        on:click=move |_| {
                            set_plugins_open.update(|v| *v = !*v);
                            close_view_menu();
                        }
                    >
                        <span class="w-3 text-[10px]">{move || if plugins_open.get() { "✓" } else { "" }}</span>
                        "Plugins"
                    </button>
                </div>
            </div>

            <div class="h-4 w-px bg-border" />

            // Actions
            <div class="flex items-center gap-1">
                <form method="post" action="/ui/action">
                    <input type="hidden" name="return_to" value=move || state.with(|s| s.return_to.clone()) />
                    <input type="hidden" name="action" value="scan" />
                    <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm attr:r#type="submit" class="text-xs h-7 px-2">
                        "Scan"
                    </Button>
                </form>

                {move || {
                    let s = state.with(|s| s.clone());
                    render_watcher(&s, set_state)
                }}

                {move || {
                    let buttons: Vec<(String, String, String, String)> = state.with(|s| {
                        s.plugin_buttons.iter().map(|btn| {
                            (btn.plugin_name.clone(), btn.action_id.clone(), btn.label.clone(), s.return_to.clone())
                        }).collect()
                    });
                    let ids = state.with(|s| s.selected_ids.join(","));
                    buttons.into_iter().map(|(plugin_name, action_id, label, return_to)| {
                        let ids = ids.clone();
                        view! {
                            <form method="post" action=format!("/plugin/{}/action", plugin_name)>
                                <input type="hidden" name="return_to" value=return_to />
                                <input type="hidden" name="plugin_action" value=action_id />
                                <input type="hidden" name="selected" value=ids />
                                <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm attr:r#type="submit" class="text-xs h-7 px-2">
                                    {label}
                                </Button>
                            </form>
                        }
                    }).collect::<Vec<_>>()
                }}
            </div>

            <ThemeToggle />
        </header>
    }
}

/// Render watcher pause/resume controls.
fn render_watcher(
    state: &UiState,
    set_state: WriteSignal<UiState>,
) -> impl IntoView + use<> {
    let watcher_paused = state.watcher_paused;
    let return_to = state.return_to.clone();
    let action_value = if watcher_paused { "resume" } else { "pause" };
    view! {
        <form
            method="post"
            action="/ui/action"
            class="flex items-center gap-1"
            data-route-action="true"
            on:change=move |event| {
                super::submit_changed_form(event, set_state);
            }
        >
            <input type="hidden" name="return_to" value=return_to />
            <input type="hidden" name="mode" value="watcher" />
            <input type="hidden" name="action" value=action_value />
            <Switch checked=!watcher_paused />
        </form>
    }
}

/// Close the View dropdown menu.
pub fn close_view_menu() {
    #[cfg(feature = "hydrate")]
    {
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if let Some(menu) = doc.get_element_by_id("view-dropdown-menu") {
                menu.remove_attribute("data-open").ok();
            }
        }
    }
}
