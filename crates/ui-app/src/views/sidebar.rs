//! Item table panel (left pane of the resizable split).

use leptos::prelude::*;

use crate::app::ItemContextMenu;
use crate::model::{ItemSummary, UiState};
use crate::route::RouteState;
use crate::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::ui::table::*;

/// Render the item table filling the left panel.
#[allow(clippy::single_call_fn)]
pub fn render_sidebar(
    state: ReadSignal<UiState>,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
    set_context_menu: WriteSignal<Option<ItemContextMenu>>,
) -> impl IntoView {
    view! {
        <TableWrapper class="border-0 rounded-none max-h-none h-full">
            <Table>
                <TableHeader>
                    <TableRow class="text-xs">
                        <TableHead class="w-8 px-2">" "</TableHead>
                        <TableHead class="px-2">"Title"</TableHead>
                        <TableHead class="px-2 w-20">"Year"</TableHead>
                        <TableHead class="px-2 hidden min-[1000px]:table-cell">"Authors"</TableHead>
                        <TableHead class="px-2 w-24">"Type"</TableHead>
                        <TableHead class="px-2 hidden min-[1200px]:table-cell min-w-24">"Tags"</TableHead>
                    </TableRow>
                </TableHeader>
                <TableBody>
                    {move || state.with(|s| {
                        s.items.iter().map(|item| {
                            render_item_row(item, s, set_state, set_events_open, set_context_menu)
                        }).collect::<Vec<_>>()
                    })}
                </TableBody>
            </Table>
        </TableWrapper>
    }
}

/// Render one library item row.
fn render_item_row(
    item: &ItemSummary,
    state: &UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
    set_context_menu: WriteSignal<Option<ItemContextMenu>>,
) -> impl IntoView + use<> {
    let id = item.id.clone();
    let check_id = item.id.clone();
    let check_state = state.clone();
    let link_state = state.clone();
    let active = state.active_id.as_deref() == Some(item.id.as_str())
        && state.selected_ids.is_empty();
    let checked = state.selected_ids.iter().any(|v| v == &item.id);
    let context_item_id = item.id.clone();
    let item_type = item.item_type.clone();
    let title = item.title.clone();
    let authors = item.authors.join("; ");
    let year = String::new(); // Year not in ItemSummary model yet; placeholder
    let category_badges: Vec<String> = item.categories.iter().take(3).cloned().collect();
    let data_title = item.title.to_ascii_lowercase();
    let data_id = item.id.to_ascii_lowercase();
    let data_authors = item.authors.join(" ").to_ascii_lowercase();
    let data_categories = item.categories.join("|");

    view! {
        <TableRow
            class=if active || checked { "bg-accent/50" } else { "" }
            attr:data-title=data_title
            attr:data-id=data_id
            attr:data-authors=data_authors
            attr:data-categories=data_categories
            on:contextmenu=move |event| {
                event.prevent_default();
                event.stop_propagation();
                set_context_menu.set(Some(ItemContextMenu {
                    item_id: context_item_id.clone(),
                    x: event.client_x(),
                    y: event.client_y(),
                }));
            }
        >
            <TableCell class="w-8 px-2 py-1.5">
                <input
                    type="checkbox"
                    class="size-3.5 accent-primary"
                    checked=checked
                    on:click=move |event| {
                        event.stop_propagation();
                        let id = check_id.clone();
                        let mut route = RouteState::from_ui_state(&check_state);
                        if route.selected_ids.iter().any(|v| v == &id) {
                            route.selected_ids.retain(|v| v != &id);
                        } else {
                            route.selected_ids.push(id);
                        }
                        super::visit_route(route, set_state, set_events_open, true);
                    }
                />
            </TableCell>
            <TableCell class="px-2 py-1.5">
                <a
                    class="text-sm text-foreground no-underline cursor-pointer truncate block"
                    href="/"
                    on:click=move |event| {
                        event.prevent_default();
                        let mut route = RouteState::from_ui_state(&link_state);
                        route.active_id = Some(id.clone());
                        route.selected_ids.clear();
                        route.tab = if link_state.tab == "files" {
                            "files".to_string()
                        } else {
                            "metadata".to_string()
                        };
                        super::visit_route(route, set_state, set_events_open, true);
                    }
                >
                    {title}
                </a>
            </TableCell>
            <TableCell class="px-2 py-1.5 text-xs text-muted-foreground w-20">
                {year}
            </TableCell>
            <TableCell class="px-2 py-1.5 text-xs text-muted-foreground truncate hidden min-[1000px]:table-cell max-w-48">
                {authors}
            </TableCell>
            <TableCell class="px-2 py-1.5 text-xs text-muted-foreground w-24">
                {item_type}
            </TableCell>
            <TableCell class="px-2 py-1.5 hidden min-[1200px]:table-cell">
                <span class="flex gap-1 flex-wrap">
                    {category_badges.into_iter().map(|c| {
                        view! { <Badge variant=BadgeVariant::Outline size=BadgeSize::Sm class="text-[10px] px-1.5 py-0 whitespace-nowrap">{c}</Badge> }
                    }).collect::<Vec<_>>()}
                </span>
            </TableCell>
        </TableRow>
    }
}
