//! Plugin management — floating dialog to enable/disable and open plugin dirs.

use leptos::prelude::*;

use crate::model::UiState;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::switch::Switch;

/// Render the plugin management page as a floating dialog overlay.
pub fn render_plugins_admin_floating(
    state: &UiState,
    set_plugins_open: WriteSignal<bool>,
) -> impl IntoView + use<> {
    let return_to = state.return_to.clone();
    let rows = state.plugins_admin.clone();

    view! {
        <div class="fixed inset-0 z-40 flex items-center justify-center">
            // Backdrop
            <div
                class="absolute inset-0 bg-black/40"
                on:click=move |_| set_plugins_open.set(false)
            />
            // Dialog
            <div class="relative z-50 w-[min(640px,calc(100vw-3rem))] max-h-[min(640px,calc(100vh-4rem))] bg-card border border-border shadow-lg flex flex-col">
                <div class="flex items-center justify-between px-4 py-2 border-b border-border shrink-0">
                    <h3 class="text-sm font-semibold">"Plugins"</h3>
                    <button
                        type="button"
                        class="text-xs text-muted-foreground hover:text-foreground px-2 py-1"
                        on:click=move |_| set_plugins_open.set(false)
                    >"Close"</button>
                </div>
                <div class="flex-1 overflow-auto p-4">
                    {if rows.is_empty() {
                        view! {
                            <p class="text-xs text-muted-foreground">
                                "No plugins discovered."
                            </p>
                        }.into_any()
                    } else {
                        view! {
                            <div class="divide-y divide-border">
                                {rows.into_iter().map(|row| {
                                    plugin_row(&row, &return_to)
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }}
                </div>
            </div>
        </div>
    }
}

/// Render one plugin management row: name, description, toggle, open-dir.
fn plugin_row(
    row: &crate::model::PluginAdminRow,
    return_to: &str,
) -> impl IntoView + use<> {
    let name = row.name.clone();
    let description = row.description.clone().unwrap_or_default();
    let enabled = row.enabled;
    // Toggling the switch posts the opposite of the current state.
    let next_enabled = if enabled { "false" } else { "true" };
    let enabled_action = format!("/plugin/{name}/enabled");
    let open_action = format!("/plugin/{name}/open-dir");
    let return_enabled = return_to.to_string();
    let return_open = return_to.to_string();
    let display_name = name.clone();

    view! {
        <div class="flex items-center gap-3 py-3">
            <div class="min-w-0 flex-1">
                <div class="text-sm font-medium truncate">{display_name}</div>
                {(!description.is_empty()).then(|| view! {
                    <div class="text-xs text-muted-foreground truncate">{description}</div>
                })}
            </div>
            // Open directory
            <form method="post" action=open_action>
                <input type="hidden" name="return_to" value=return_open />
                <Button variant=ButtonVariant::Outline size=ButtonSize::Sm attr:r#type="submit" class="text-xs h-7 px-2">
                    "Open dir"
                </Button>
            </form>
            // Enable / disable toggle
            <form method="post" action=enabled_action>
                <input type="hidden" name="return_to" value=return_enabled />
                <input type="hidden" name="enabled" value=next_enabled />
                <button type="submit" class="inline-flex" title=if enabled { "Disable" } else { "Enable" }>
                    <Switch checked=enabled />
                </button>
            </form>
        </div>
    }
}
