//! Item data grid panel (left pane of the resizable split).

use std::sync::LazyLock;

use leptos::prelude::*;
use strum::{AsRefStr, Display};

use crate::app::ItemContextMenu;
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::data_grid::{
    DataGridColumn, DataGridToolbar, GridWrapper, PinnableColumn, TableSeparator, generate_grid_style,
};
use crate::model::{ItemSummary, UiState};
use crate::route::{RouteState, optional_text};

/* ========================================================== */
/*                     COLUMN DEFINITION                       */
/* ========================================================== */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, AsRefStr)]
pub enum Column {
    Title,
    Year,
    Authors,
    Type,
    Tags,
}

impl PinnableColumn for Column {
    fn pinnable_columns() -> &'static [(Self, i32)] {
        &[
            (Self::Title, 280),
            (Self::Year, 60),
            (Self::Authors, 180),
            (Self::Type, 90),
            (Self::Tags, 160),
        ]
    }
}

impl DataGridColumn for Column {
    fn colindex(self) -> i32 {
        self as i32 + 2
    }
}

/// Precomputed CSS custom properties for the grid's column widths.
static GRID_STYLE: LazyLock<String> = LazyLock::new(generate_grid_style::<Column>);

/* ========================================================== */
/*                     RENDER SIDEBAR                          */
/* ========================================================== */

/// Render the item data grid filling the left panel.
#[allow(clippy::single_call_fn)]
pub fn render_sidebar(
    state: ReadSignal<UiState>,
    set_state: WriteSignal<UiState>,
    set_context_menu: WriteSignal<Option<ItemContextMenu>>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col h-full" data-name="ItemDataGrid">
            // Toolbar: unified search + category
            <DataGridToolbar class="px-2 py-1 border-b mb-0 gap-2">
                <input
                    id="grid-filter"
                    name="q"
                    placeholder="Search or filter..."
                    class="h-6 flex-1 min-w-0 border border-input bg-transparent px-2 text-xs outline-none focus-visible:border-ring focus-visible:ring-1 focus-visible:ring-ring/50"
                    value=move || state.with(|s| s.search.clone().unwrap_or_default())
                    on:change=move |event| {
                        let value = event_target_value(&event);
                        state.with(|s| {
                            let mut route = RouteState::from_ui_state(s);
                            route.search = optional_text(&value);
                            route.active_id = None;
                            route.selected_ids.clear();
                            super::visit_route(route, set_state, true);
                        });
                    }
                />
                <select
                    id="library-category"
                    name="category"
                    class="h-6 border border-input bg-transparent px-1 text-xs outline-none focus-visible:border-ring focus-visible:ring-1 focus-visible:ring-ring/50"
                    on:change=move |event| {
                        let value = event_target_value(&event);
                        state.with(|s| {
                            let mut route = RouteState::from_ui_state(s);
                            route.category = optional_text(&value);
                            route.active_id = None;
                            route.selected_ids.clear();
                            super::visit_route(route, set_state, true);
                        });
                    }
                >
                    <option value="" selected=move || state.with(|s| s.category.is_none())>"All"</option>
                    {move || state.with(|s| {
                        s.categories.iter().map(|cat| {
                            let selected = s.category.as_deref() == Some(cat.path.as_str());
                            view! { <option value=cat.path.clone() selected=selected>{cat.path.clone()}</option> }
                        }).collect::<Vec<_>>()
                    })}
                </select>
                <span id="grid-filter-count" class="text-xs text-muted-foreground whitespace-nowrap">
                    {move || state.with(|s| format!("{}", s.items.len()))}
                </span>
            </DataGridToolbar>

            <GridWrapper class="flex-1 rounded-none border-0 max-h-none h-full overflow-hidden">
                <div
                    role="grid"
                    data-name="DataGrid"
                    class="grid overflow-y-auto overflow-x-hidden relative border-0 focus:outline-none h-full"
                    aria-label="Item library"
                    style=GRID_STYLE.as_str()
                >
                    // Header with sort/pin dropdowns + resize handles
                    <div role="rowgroup" data-slot="grid-header" class="grid sticky top-0 z-10 border-b bg-background">
                        <div role="row" aria-rowindex="1" data-slot="grid-header-row" tabindex="-1" class="flex w-full">
                            <div
                                role="columnheader"
                                aria-colindex="1"
                                tabindex="-1"
                                class="relative"
                                style="width: calc(var(--header-Select-size) * 1px);"
                            >
                                <div class="py-1 px-2 size-full">
                                    <span class="text-xs text-muted-foreground">" "</span>
                                </div>
                            </div>
                            {Column::pinnable_columns().iter().map(|(col, width)| {
                                let css_name = col.css_safe_name();
                                let colindex = col.colindex();
                                let label = col.to_string();
                                let col = *col;
                                let width = *width;
                                let is_title = col == Column::Title;
                                let cell_class = if is_title {
                                    "relative border-r bg-background flex-1 min-w-0"
                                } else {
                                    "relative border-r bg-background"
                                };
                                let cell_style = if is_title {
                                    format!("min-width: calc(var(--header-{css_name}-size) * 1px);")
                                } else {
                                    format!("width: calc(var(--header-{css_name}-size) * 1px);")
                                };
                                view! {
                                    <div
                                        role="columnheader"
                                        aria-sort="none"
                                        aria-colindex=colindex.to_string()
                                        class=cell_class
                                        tabindex="-1"
                                        style=cell_style
                                    >
                                        <div class="flex gap-1 justify-between items-center px-2 py-1 w-full h-full text-xs font-medium text-muted-foreground">
                                            <span class="truncate">{label}</span>
                                        </div>
                                        <TableSeparator valuenow=width />
                                    </div>
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    </div>

                    // Body
                    <div role="rowgroup" data-name="GridBody" class="grid relative">
                        {move || {
                            state.with(|s| {
                                let base_route = RouteState::from_ui_state(s);
                                s.items.iter().enumerate().map(|(idx, item)| {
                                    render_item_row(
                                        idx, item, &base_route, set_state, set_context_menu,
                                    )
                                }).collect::<Vec<_>>()
                            })
                        }}
                    </div>
                </div>
            </GridWrapper>

            <script>
                {r#"
                (function() {
                    const setup = () => {
                        const input = document.getElementById('grid-filter');
                        const countEl = document.getElementById('grid-filter-count');
                        if (!input) { setTimeout(setup, 50); return; }
                        if (input.hasAttribute('data-filter-init')) return;
                        input.setAttribute('data-filter-init', 'true');

                        input.addEventListener('input', function() {
                            const q = this.value.toLowerCase().trim();
                            const grid = document.querySelector('[data-name="GridBody"]');
                            if (!grid) return;
                            const rows = grid.querySelectorAll('[role="row"]');
                            let visible = 0;
                            rows.forEach(function(row) {
                                if (!q) { row.style.display = ''; visible++; return; }
                                const title = row.getAttribute('data-title') || '';
                                const id = row.getAttribute('data-id') || '';
                                const authors = row.getAttribute('data-authors') || '';
                                const categories = row.getAttribute('data-categories') || '';
                                const match = title.includes(q) || id.includes(q) || authors.includes(q) || categories.includes(q);
                                row.style.display = match ? '' : 'none';
                                if (match) visible++;
                            });
                            if (countEl) countEl.textContent = visible;
                        });

                        // Column resize via TableSeparator drag
                        const grid = document.querySelector('[data-name="DataGrid"]');
                        if (!grid) return;
                        const separators = grid.querySelectorAll('[role="separator"]');
                        separators.forEach(function(sep) {
                            let startX = 0;
                            let startWidth = 0;
                            const headerCell = sep.closest('[role="columnheader"]');
                            if (!headerCell) return;

                            const onMouseMove = function(e) {
                                const diff = e.clientX - startX;
                                const newWidth = Math.max(40, startWidth + diff);
                                const widthStyle = headerCell.getAttribute('style') || '';
                                const match = widthStyle.match(/--header-(\w+)-size/);
                                if (match) {
                                    const colName = match[1];
                                    grid.style.setProperty('--header-' + colName + '-size', newWidth);
                                    grid.style.setProperty('--col-' + colName + '-size', newWidth);
                                }
                            };

                            const onMouseUp = function() {
                                document.removeEventListener('mousemove', onMouseMove);
                                document.removeEventListener('mouseup', onMouseUp);
                                document.body.style.cursor = '';
                                document.body.style.userSelect = '';
                            };

                            sep.addEventListener('mousedown', function(e) {
                                e.preventDefault();
                                startX = e.clientX;
                                const widthStyle = headerCell.getAttribute('style') || '';
                                const match = widthStyle.match(/--header-(\w+)-size/);
                                if (match) {
                                    const colName = match[1];
                                    const computed = getComputedStyle(grid).getPropertyValue('--header-' + colName + '-size');
                                    startWidth = parseInt(computed) || parseInt(sep.getAttribute('aria-valuenow')) || 150;
                                }
                                document.body.style.cursor = 'ew-resize';
                                document.body.style.userSelect = 'none';
                                document.addEventListener('mousemove', onMouseMove);
                                document.addEventListener('mouseup', onMouseUp);
                            });
                        });
                    };
                    if (document.readyState === 'loading') {
                        document.addEventListener('DOMContentLoaded', setup);
                    } else {
                        setup();
                    }
                })();
                "#}
            </script>
        </div>
    }
}

/* ========================================================== */
/*                     ROW RENDERING                           */
/* ========================================================== */

/// Render a single item row in the data grid body.
#[allow(clippy::single_call_fn)]
fn render_item_row(
    idx: usize,
    item: &ItemSummary,
    base_route: &RouteState,
    set_state: WriteSignal<UiState>,
    set_context_menu: WriteSignal<Option<ItemContextMenu>>,
) -> impl IntoView + use<> {
    let check_id = item.id.clone();
    let check_route = base_route.clone();
    let link_id = item.id.clone();
    let link_route = base_route.clone();
    let context_id = item.id.clone();
    let title = item.title.clone();
    let authors = item.authors.join("; ");
    let item_type = item.item_type.clone();
    let category_badges: Vec<String> = item.categories.iter().take(3).cloned().collect();
    let active = base_route.active_id.as_deref() == Some(item.id.as_str())
        && base_route.selected_ids.is_empty();
    let checked = base_route.selected_ids.iter().any(|v| v == &item.id);

    let data_title = item.title.to_ascii_lowercase();
    let data_id = item.id.to_ascii_lowercase();
    let data_authors = item.authors.join(" ").to_ascii_lowercase();
    let data_categories = item.categories.join("|");

    let row_class = if active || checked { "hover:bg-muted/50 bg-accent/50" } else { "hover:bg-muted/50" };

    view! {
        <div
            role="row"
            data-name="GridRow"
            aria-rowindex=(idx + 2).to_string()
            class=format!("flex w-full border-b shrink-0 overflow-hidden {row_class}")
            tabindex="-1"
            style="height: 30px; max-height: 30px;"
            data-title=data_title
            data-id=data_id
            data-authors=data_authors
            data-categories=data_categories
            on:contextmenu=move |event| {
                event.prevent_default();
                event.stop_propagation();
                set_context_menu.set(Some(ItemContextMenu {
                    item_id: context_id.clone(),
                    x: event.client_x(),
                    y: event.client_y(),
                }));
            }
        >
            // Select checkbox
            <div
                role="gridcell"
                aria-colindex="1"
                tabindex="-1"
                class="flex items-center"
                style="width: calc(var(--col-Select-size) * 1px);"
            >
                <div class="py-1 px-2">
                    <input
                        type="checkbox"
                        class="size-3.5 accent-primary"
                        checked=checked
                        on:click=move |event| {
                            event.stop_propagation();
                            let id = check_id.clone();
                            let mut route = check_route.clone();
                            if route.selected_ids.iter().any(|v| v == &id) {
                                route.selected_ids.retain(|v| v != &id);
                            } else {
                                route.selected_ids.push(id);
                            }
                            super::visit_route(route, set_state, true);
                        }
                    />
                </div>
            </div>

            // Title
            <div
                role="gridcell"
                aria-colindex=Column::Title.colindex().to_string()
                class="relative border-r bg-background flex items-center overflow-hidden flex-1 min-w-0"
                style="min-width: calc(var(--col-Title-size) * 1px);"
            >
                <a
                    class="text-xs text-foreground no-underline cursor-pointer truncate block px-2"
                    href="/"
                    on:click=move |event| {
                        event.prevent_default();
                        let mut route = link_route.clone();
                        route.active_id = Some(link_id.clone());
                        route.selected_ids.clear();
                        route.tab = if route.tab == "files" { "files".to_string() } else { "metadata".to_string() };
                        super::visit_route(route, set_state, true);
                    }
                >
                    {title}
                </a>
            </div>

            {text_cell(Column::Year, String::new())}
            {text_cell(Column::Authors, authors)}
            {text_cell(Column::Type, item_type)}

            // Tags
            <div
                role="gridcell"
                aria-colindex=Column::Tags.colindex().to_string()
                class="relative border-r bg-background flex items-center overflow-hidden shrink-0"
                style="width: calc(var(--col-Tags-size) * 1px);"
            >
                <span class="flex gap-0.5 flex-wrap px-1.5">
                    {category_badges.into_iter().map(|c| {
                        view! { <Badge variant=BadgeVariant::Outline size=BadgeSize::Sm class="text-[10px] px-1 py-0 whitespace-nowrap">{c}</Badge> }
                    }).collect::<Vec<_>>()}
                </span>
            </div>
        </div>
    }
}

/// Render a plain text body cell for a fixed-width column.
fn text_cell(col: Column, content: String) -> impl IntoView {
    let css_name = col.css_safe_name();
    view! {
        <div
            role="gridcell"
            aria-colindex=col.colindex().to_string()
            class="relative border-r bg-background flex items-center overflow-hidden shrink-0"
            style=format!("width: calc(var(--col-{css_name}-size) * 1px);")
        >
            <span class="text-xs text-muted-foreground truncate px-2">{content}</span>
        </div>
    }
}
