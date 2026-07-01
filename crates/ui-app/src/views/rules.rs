//! Rules editor — floating dialog + notice toast.

use leptos::prelude::*;

use crate::model::{RulesNotice, UiState};
use crate::components::ui::button::{Button, ButtonVariant, ButtonSize};

/// Render rules editor as a floating dialog overlay.
pub fn render_rules_floating(
    state: &UiState,
    set_state: WriteSignal<UiState>,
    set_rules_open: WriteSignal<bool>,
) -> impl IntoView + use<> {
    let return_to = state.return_to.clone();
    let rules_text = state.rules_text.clone();

    view! {
        <div class="fixed inset-0 z-40 flex items-center justify-center">
            // Backdrop
            <div
                class="absolute inset-0 bg-black/40"
                on:click=move |_| set_rules_open.set(false)
            />
            // Dialog
            <div class="relative z-50 w-[min(700px,calc(100vw-3rem))] max-h-[min(700px,calc(100vh-4rem))] bg-card border border-border shadow-lg flex flex-col">
                <div class="flex items-center justify-between px-4 py-2 border-b border-border shrink-0">
                    <h3 class="text-sm font-semibold">"Rules Editor"</h3>
                    <button
                        type="button"
                        class="text-xs text-muted-foreground hover:text-foreground px-2 py-1"
                        on:click=move |_| set_rules_open.set(false)
                    >"Close"</button>
                </div>
                <div class="flex-1 overflow-auto p-4">
                    <form
                        method="post" action="/ui/action"
                        class="grid gap-3 h-full"
                        data-route-action="true"
                        on:submit=move |event| {
                            event.prevent_default();
                            super::submit_action(event, set_state);
                        }
                    >
                        <input type="hidden" name="return_to" value=return_to />
                        <input type="hidden" name="action" value="save_rules" />
                        <textarea
                            name="rules_text"
                            class="flex-1 min-h-[400px] w-full border border-input bg-transparent px-3 py-2 font-mono text-xs outline-none focus-visible:border-ring focus-visible:ring-1 focus-visible:ring-ring/50 resize-y"
                        >{rules_text}</textarea>
                        <Button variant=ButtonVariant::Default size=ButtonSize::Sm attr:r#type="submit">
                            "Save Rules"
                        </Button>
                    </form>
                </div>
            </div>
        </div>
    }
}

/// Render rules in the detail panel tab (kept for route compatibility).
pub fn render_rules(
    state: &UiState,
    set_state: WriteSignal<UiState>,
) -> impl IntoView + use<> {
    let return_to = state.return_to.clone();
    let rules_text = state.rules_text.clone();

    view! {
        <div class="p-4">
            <form
                method="post" action="/ui/action"
                class="grid gap-3"
                data-route-action="true"
                on:submit=move |event| {
                    event.prevent_default();
                    super::submit_action(event, set_state);
                }
            >
                <input type="hidden" name="return_to" value=return_to />
                <input type="hidden" name="action" value="save_rules" />
                <textarea
                    name="rules_text"
                    class="min-h-[400px] w-full border border-input bg-transparent px-3 py-2 font-mono text-xs outline-none focus-visible:border-ring focus-visible:ring-1 focus-visible:ring-ring/50 resize-y"
                >{rules_text}</textarea>
                <Button variant=ButtonVariant::Default size=ButtonSize::Sm attr:r#type="submit">
                    "Save Rules"
                </Button>
            </form>
        </div>
    }
}

/// Render the rules notice toast (success or error).
pub fn render_rules_notice(
    state: UiState,
    set_state: WriteSignal<UiState>,
) -> AnyView {
    match state.rules_notice {
        Some(RulesNotice::Saved { rules }) => view! {
            <aside class="fixed right-4 bottom-4 z-50 w-[min(400px,calc(100vw-2rem))] max-h-[min(400px,calc(100vh-2rem))] overflow-auto border border-border bg-card shadow-lg" role="status">
                <div class="flex items-center justify-between border-b border-border px-3 py-2">
                    <span class="text-xs font-semibold">"Parsed Rules"</span>
                    <button
                        type="button"
                        class="text-xs text-muted-foreground hover:text-foreground px-2 py-0.5"
                        on:click=move |event| {
                            event.prevent_default();
                            dismiss_rules_notice(set_state);
                        }
                    >"Close"</button>
                </div>
                <div class="divide-y divide-border">
                    {rules.into_iter().map(|rule| view! {
                        <div class="grid gap-0.5 px-3 py-2">
                            <strong class="text-xs">{rule.name}</strong>
                            <span class="text-[10px] text-muted-foreground">{rule.target}</span>
                            <code class="text-[10px] bg-muted px-1.5 py-0.5 overflow-auto font-mono">{rule.query}</code>
                        </div>
                    }).collect::<Vec<_>>()}
                </div>
            </aside>
        }.into_any(),
        Some(RulesNotice::Error { message }) => view! {
            <aside class="fixed right-4 bottom-4 z-50 w-[min(400px,calc(100vw-2rem))] border border-destructive bg-card shadow-lg" role="alert">
                <div class="flex items-center justify-between border-b border-border px-3 py-2">
                    <span class="text-xs font-semibold text-destructive">"Rules Error"</span>
                    <button
                        type="button"
                        class="text-xs text-muted-foreground hover:text-foreground px-2 py-0.5"
                        on:click=move |event| {
                            event.prevent_default();
                            dismiss_rules_notice(set_state);
                        }
                    >"Close"</button>
                </div>
                <p class="px-3 py-2 text-xs">{message}</p>
            </aside>
        }.into_any(),
        None => ().into_any(),
    }
}

/// Dismiss the rules notice.
fn dismiss_rules_notice(set_state: WriteSignal<UiState>) {
    set_state.update(|state| state.rules_notice = None);
    #[cfg(feature = "hydrate")]
    crate::client::clear_rules_notice_query();
}
