//! Item context menu (right-click).

use leptos::prelude::*;

use crate::app::ItemContextMenu;
use crate::model::{ItemSummary, UiState};

/// Render the item context menu.
pub fn render_item_context_menu(
    state: &UiState,
    menu: Option<ItemContextMenu>,
    set_state: WriteSignal<UiState>,
    set_context_menu: WriteSignal<Option<ItemContextMenu>>,
) -> AnyView {
    let Some(menu) = menu else { return ().into_any() };
    let Some(item) = state.items.iter().find(|i| i.id == menu.item_id) else {
        return ().into_any();
    };
    let files = context_menu_files(item);
    let item_id = item.id.clone();
    let return_to = state.return_to.clone();
    let left = format!("{}px", menu.x);
    let top = format!("{}px", menu.y);

    view! {
        <aside
            class="fixed z-50 w-[min(280px,calc(100vw-24px))] rounded-lg border border-border bg-card shadow-lg overflow-hidden"
            style:left=left
            style:top=top
            role="menu"
            on:click=move |event| event.stop_propagation()
        >
            // Files section
            <div class="border-b border-border">
                <h3 class="px-3 py-2 text-xs font-medium uppercase text-muted-foreground">"Files"</h3>
                <div class="max-h-48 overflow-auto">
                    {if files.is_empty() {
                        view! { <p class="px-3 py-2 text-sm text-muted-foreground">"No files"</p> }.into_any()
                    } else {
                        files.into_iter().map(|path| {
                            let open_item_id = item_id.clone();
                            let open_return_to = return_to.clone();
                            view! {
                                <form
                                    method="post" action="/ui/action"
                                    data-route-action="true"
                                    on:submit=move |event| {
                                        event.prevent_default();
                                        set_context_menu.set(None);
                                        super::submit_action(event, set_state);
                                    }
                                >
                                    <input type="hidden" name="return_to" value=open_return_to />
                                    <input type="hidden" name="action" value="open_file" />
                                    <input type="hidden" name="item_id" value=open_item_id />
                                    <input type="hidden" name="file_path" value=path.clone() />
                                    <button type="submit" class="w-full text-left px-3 py-2 text-sm hover:bg-muted transition truncate">
                                        {path}
                                    </button>
                                </form>
                            }
                        }).collect::<Vec<_>>().into_any()
                    }}
                </div>
            </div>

            // Item actions
            <div class="border-b border-border">
                <h3 class="px-3 py-2 text-xs font-medium uppercase text-muted-foreground">"Item"</h3>
                <form
                    method="post" action="/ui/action"
                    data-route-action="true"
                    on:submit=move |event| {
                        event.prevent_default();
                        set_context_menu.set(None);
                        super::submit_action(event, set_state);
                    }
                >
                    <input type="hidden" name="return_to" value=return_to.clone() />
                    <input type="hidden" name="action" value="delete_item" />
                    <input type="hidden" name="item_id" value=item_id.clone() />
                    <button type="submit" class="w-full text-left px-3 py-2 text-sm text-destructive hover:bg-destructive/10 transition">
                        "Delete Item"
                    </button>
                </form>
            </div>

            // Plugin menu items
            {render_context_plugin_items(state, &menu, &return_to, set_state, set_context_menu)}
        </aside>
    }.into_any()
}

/// Plugin actions in context menu.
fn render_context_plugin_items(
    state: &UiState,
    menu: &ItemContextMenu,
    return_to: &str,
    set_state: WriteSignal<UiState>,
    set_context_menu: WriteSignal<Option<ItemContextMenu>>,
) -> AnyView {
    if state.plugin_menu_items.is_empty() {
        return ().into_any();
    }
    let plugin_items = state.plugin_menu_items.clone();
    let menu_item_id = menu.item_id.clone();
    let return_to = return_to.to_string();

    view! {
        <div>
            <h3 class="px-3 py-2 text-xs font-medium uppercase text-muted-foreground">"Plugins"</h3>
            {plugin_items.into_iter().map(|pi| {
                let rt = return_to.clone();
                let mid = menu_item_id.clone();
                view! {
                    <form
                        method="post"
                        action=format!("/plugin/{}/action", pi.plugin_name)
                        on:submit=move |event| {
                            event.prevent_default();
                            set_context_menu.set(None);
                            super::submit_action(event, set_state);
                        }
                    >
                        <input type="hidden" name="return_to" value=rt />
                        <input type="hidden" name="plugin_action" value=pi.action_id />
                        <input type="hidden" name="selected" value=mid />
                        <button type="submit" class="w-full text-left px-3 py-2 text-sm hover:bg-muted transition">
                            {pi.label}
                        </button>
                    </form>
                }
            }).collect::<Vec<_>>()}
        </div>
    }.into_any()
}

/// Files for context menu.
fn context_menu_files(item: &ItemSummary) -> Vec<String> {
    if item.files.is_empty() {
        item.main_file.iter().cloned().collect()
    } else {
        item.files.clone()
    }
}
