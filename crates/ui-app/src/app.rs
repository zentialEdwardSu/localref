//! Shared Leptos component tree for the Localref web UI.

use leptos::prelude::*;

use crate::model::{
    CategorySummary, FileEntry, ItemSummary, RulesNotice, UiState,
};
use crate::route::{RouteState, optional_text};

/// Render the full HTML document around the hydratable body app.
#[cfg(feature = "ssr")]
pub fn document(initial_state: UiState) -> impl IntoView {
    let title = initial_state.repo_name.clone();
    view! {
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <title>{title}</title>
                <link rel="icon" href="/assets/favicon.ico" type="image/x-icon"/>
                <link rel="stylesheet" href="/assets/localref-ui.css"/>
                <script type="module" src="/assets/localref-ui.js"></script>
            </head>
            <body>{body_app(initial_state)}</body>
        </html>
    }
}

/// Render the body children that are hydrated in the browser.
pub fn body_app(initial_state: UiState) -> impl IntoView {
    let initial_events_open = initial_state.tab == "events";
    let (state, set_state) = signal(initial_state);
    let (events_open, set_events_open) = signal(initial_events_open);
    let (context_menu, set_context_menu) =
        signal::<Option<ItemContextMenu>>(None);
    view! {
        {move || state.with(|state| state_json_script(state.clone()))}
        {move || state.with(|state| render_rules_notice(state.clone(), set_state))}
        {move || state.with(|state| {
            render_item_context_menu(
                state.clone(),
                context_menu.get(),
                set_state,
                set_events_open,
                set_context_menu,
            )
        })}
        <main
            class="app-shell"
            on:click=move |_| set_context_menu.set(None)
        >
            {render_topbar(state, set_state, events_open, set_events_open)}
            <section class="workspace">
                {render_sidebar(
                    state,
                    set_state,
                    set_events_open,
                    set_context_menu,
                )}
                <section class="detail-pane">
                    <div data-primary-detail="true" hidden=move || events_open.get()>
                        <div class="detail-head" data-primary-detail-head="true">
                            <div class="title-block">
                                <h2>{move || state.with(detail_title)}</h2>
                            </div>
                            <nav class="right-tabs">
                                {move || state.with(|state| {
                                    let snapshot = state.clone();
                                    if snapshot.selected_ids.is_empty() {
                                        view! {
                                            {route_tab_button("Metadata", "metadata", snapshot.clone(), set_state, set_events_open)}
                                            {route_tab_button("Files", "files", snapshot, set_state, set_events_open)}
                                        }.into_any()
                                    } else {
                                        view! {}.into_any()
                                    }
                                })}
                                {move || state.with(|state| {
                                    route_tab_button("Rules", "rules", state.clone(), set_state, set_events_open)
                                })}
                            </nav>
                        </div>
                        <div class="detail-body">
                            {move || state.with(|state| render_detail(state.clone(), set_state, set_events_open))}
                        </div>
                    </div>
                    <section class="event-panel" hidden=move || !events_open.get()>
                        {move || state.with(|state| render_events(state.clone()))}
                    </section>
                </section>
            </section>
        </main>
    }
}

#[derive(Clone, Debug)]
struct ItemContextMenu {
    item_id: String,
    x: i32,
    y: i32,
}

/// Render a JSON script containing the state used by hydration.
fn state_json_script(state: UiState) -> impl IntoView {
    let state_json = serde_json::to_string(&state)
        .expect("Localref UI state should serialize")
        .replace('<', "\\u003C");
    view! {
        <script id="localref-ui-state" type="application/json">
            {state_json}
        </script>
    }
}

/// Render the top bar and daemon controls.
fn render_topbar(
    state: ReadSignal<UiState>,
    set_state: WriteSignal<UiState>,
    events_open: ReadSignal<bool>,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView {
    view! {
        <header class="topbar">
            <div class="brand-block">
                <div class="brand-row">
                    <h1>{move || state.with(|state| state.repo_name.clone())}</h1>
                    <span class="status-pill">{move || state.with(|state| state.status_label.clone())}</span>
                </div>
            </div>
            <div class="top-actions">
                <div class="stats">
                    <span>{move || state.with(|state| state.items.len())} " items"</span>
                    <span>{move || state.with(|state| state.categories.len())} " categories"</span>
                    <span>{move || state.with(|state| state.events.len())} " events"</span>
                    <span>{move || state.with(|state| state.pending_count)} " pending"</span>
                </div>
                <div class="control-row">
                    <button
                        class=move || top_event_class(events_open.get())
                        type="button"
                        data-events-toggle="true"
                        aria-pressed=move || events_open.get().to_string()
                        on:click=move |event| {
                            event.prevent_default();
                            set_events_open.update(|open| *open = !*open);
                        }
                    >
                        "Events"
                    </button>
                    <form method="post" action="/ui/action">
                        <input type="hidden" name="return_to" value=move || state.with(|state| state.return_to.clone())/>
                        <input type="hidden" name="action" value="scan"/>
                        <button class="button secondary" type="submit">"Run Scan"</button>
                    </form>
                    {move || state.with(|state| {
                        watcher_form(state.clone(), set_state, set_events_open)
                    })}
                </div>
            </div>
        </header>
    }
}

/// Render watcher pause/resume controls.
fn watcher_form(
    state: UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView {
    let watcher_paused = state.watcher_paused;
    view! {
        <form
            method="post"
            action="/ui/action"
            class="watcher-form"
            data-route-action="true"
            on:change=move |event| {
                submit_changed_form(event, set_state, set_events_open);
            }
        >
            <input type="hidden" name="return_to" value=state.return_to.clone()/>
            <input type="hidden" name="mode" value="watcher"/>
            <div class="radio-pair">
                <label class=watcher_class(!watcher_paused)>
                    <input type="radio" name="action" value="resume" checked=!watcher_paused/>
                    <span>"Watcher On"</span>
                </label>
                <label class=watcher_class(watcher_paused)>
                    <input type="radio" name="action" value="pause" checked=watcher_paused/>
                    <span>"Watcher Paused"</span>
                </label>
            </div>
            <button class="sr-only" type="submit">"Apply Watcher"</button>
        </form>
    }
}

/// Render the library sidebar with filters and item rows.
fn render_sidebar(
    state: ReadSignal<UiState>,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
    set_context_menu: WriteSignal<Option<ItemContextMenu>>,
) -> impl IntoView {
    view! {
        <aside class="library-pane">
            <div class="filter-panel">
                <label class="field">
                    <span>"Search"</span>
                    <input
                        id="library-search"
                        name="q"
                        value=move || state.with(|state| state.search.clone().unwrap_or_default())
                        on:change=move |event| {
                            let value = event_target_value(&event);
                            state.with(|state| {
                                let mut route = RouteState::from_ui_state(state);
                                route.search = optional_text(&value);
                                route.active_id = None;
                                route.selected_ids.clear();
                                visit_route(route, set_state, set_events_open, true);
                            });
                        }
                    />
                </label>
                <label class="field">
                    <span>"Category Filter"</span>
                    <select
                        id="library-category"
                        name="category"
                        on:change=move |event| {
                            let value = event_target_value(&event);
                            state.with(|state| {
                                let mut route = RouteState::from_ui_state(state);
                                route.category = optional_text(&value);
                                route.active_id = None;
                                route.selected_ids.clear();
                                visit_route(route, set_state, set_events_open, true);
                            });
                        }
                    >
                        <option value="" selected=move || state.with(|state| state.category.is_none())>"All"</option>
                        {move || state.with(|state| {
                            state.categories.iter().map(|category| {
                                let selected = state.category.as_deref() == Some(category.path.as_str());
                                view! {
                                    <option value=category.path.clone() selected=selected>
                                        {category.path.clone()}
                                    </option>
                                }
                            }).collect::<Vec<_>>()
                        })}
                    </select>
                </label>
            </div>
            <form method="get" action="/" class="selection-form">
                <div id="library-list" class="item-list">
                    {move || state.with(|state| {
                        state.items.iter().cloned().map(|item| {
                            render_item_row(
                                item,
                                state.clone(),
                                set_state,
                                set_events_open,
                                set_context_menu,
                            )
                        }).collect::<Vec<_>>()
                    })}
                </div>
            </form>
        </aside>
    }
}

/// Render one library item row.
fn render_item_row(
    item: ItemSummary,
    state: UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
    set_context_menu: WriteSignal<Option<ItemContextMenu>>,
) -> impl IntoView {
    let id = item.id.clone();
    let check_id = item.id.clone();
    let check_state = state.clone();
    let link_state = state.clone();
    let active = state.active_id.as_deref() == Some(item.id.as_str())
        && state.selected_ids.is_empty();
    let checked = state.selected_ids.iter().any(|value| value == &item.id);
    let route_tab = if state.tab == "files" { "files" } else { "metadata" };
    let mut item_route = RouteState::from_ui_state(&state);
    item_route.active_id = Some(item.id.clone());
    item_route.selected_ids.clear();
    item_route.tab = route_tab.to_string();
    let href = item_route.to_path();
    let open_file = item.main_file.clone().unwrap_or_default();
    let context_item_id = item.id.clone();
    view! {
        <div
            class=if active || checked { "item-row is-active" } else { "item-row" }
            data-title=item.title.to_ascii_lowercase()
            data-id=item.id.to_ascii_lowercase()
            data-authors=item.authors.join(" ").to_ascii_lowercase()
            data-categories=item.categories.join("|")
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
            <input
                class="row-check"
                type="checkbox"
                name="item"
                value=check_id.clone()
                checked=checked
                on:click=move |event| {
                    event.stop_propagation();
                    let id = check_id.clone();
                    let mut route = RouteState::from_ui_state(&check_state);
                    if route.selected_ids.iter().any(|value| value == &id) {
                        route.selected_ids.retain(|value| value != &id);
                    } else {
                        route.selected_ids.push(id);
                    }
                    visit_route(route, set_state, set_events_open, true);
                }
            />
            <a
                class="item-link"
                href=href
                data-route-active=id.clone()
                data-route-tab=route_tab
                data-open-file=open_file.clone()
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
                    visit_route(route, set_state, set_events_open, true);
                }
            >
                <strong>{item.title.clone()}</strong>
                <span>{item_subtitle(&item)}</span>
            </a>
        </div>
    }
}

/// Render the item context menu opened from the library list.
fn render_item_context_menu(
    state: UiState,
    menu: Option<ItemContextMenu>,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
    set_context_menu: WriteSignal<Option<ItemContextMenu>>,
) -> AnyView {
    let Some(menu) = menu else {
        return view! {}.into_any();
    };
    let Some(item) = state.items.iter().find(|item| item.id == menu.item_id)
    else {
        return view! {}.into_any();
    };
    let files = if item.files.is_empty() {
        item.main_file.iter().cloned().collect::<Vec<_>>()
    } else {
        item.files.clone()
    };
    let item_id = item.id.clone();
    let return_to = state.return_to.clone();
    let has_any_file = !files.is_empty();
    let left = format!("{}px", menu.x);
    let top = format!("{}px", menu.y);
    view! {
        <aside
            class="item-context-menu"
            style:left=left
            style:top=top
            role="menu"
            on:click=move |event| event.stop_propagation()
        >
            <section>
                <h3>"Files"</h3>
                <div class="context-file-list">
                    {files.into_iter().map(|path| {
                        let display = path.clone();
                        let open_item_id = item_id.clone();
                        let open_return_to = return_to.clone();
                        view! {
                            <form
                                method="post"
                                action="/ui/action"
                                class="context-file-row"
                                data-route-action="true"
                                on:submit=move |event| {
                                    event.prevent_default();
                                    set_context_menu.set(None);
                                    submit_action(event, set_state, set_events_open);
                                }
                            >
                                <input type="hidden" name="return_to" value=open_return_to/>
                                <input type="hidden" name="action" value="open_file"/>
                                <input type="hidden" name="item_id" value=open_item_id/>
                                <input type="hidden" name="file_path" value=path/>
                                <button class="context-menu-button" type="submit">{display}</button>
                            </form>
                        }
                    }).collect::<Vec<_>>()}
                    {if has_any_file {
                        view! {}.into_any()
                    } else {
                        view! { <p>"No files"</p> }.into_any()
                    }}
                </div>
            </section>
            <section>
                <h3>"Item"</h3>
                <form
                    method="post"
                    action="/ui/action"
                    data-route-action="true"
                    on:submit=move |event| {
                        event.prevent_default();
                        set_context_menu.set(None);
                        submit_action(event, set_state, set_events_open);
                    }
                >
                    <input type="hidden" name="return_to" value=return_to/>
                    <input type="hidden" name="action" value="delete_item"/>
                    <input type="hidden" name="item_id" value=item_id/>
                    <button class="context-menu-button danger" type="submit">"Delete Item"</button>
                </form>
            </section>
        </aside>
    }
    .into_any()
}

/// Render one right-pane route tab.
fn route_tab_button(
    label: &'static str,
    tab: &'static str,
    state: UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView {
    let route_state = state.clone();
    let mut route = RouteState::from_ui_state(&state);
    route.tab = tab.to_string();
    let href = route.to_path();
    view! {
        <a
            class=right_tab_class(&state.tab, tab)
            href=href
            data-route-tab=tab
            on:click=move |event| {
                event.prevent_default();
                let mut route = RouteState::from_ui_state(&route_state);
                route.tab = tab.to_string();
                visit_route(route, set_state, set_events_open, true);
            }
        >
            {label}
        </a>
    }
}

/// Render the active detail body.
fn render_detail(
    state: UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> AnyView {
    if !state.selected_ids.is_empty() && state.tab != "rules" {
        return render_metadata(state, set_state, set_events_open);
    }
    match state.tab.as_str() {
        "files" => render_files(&state, set_state, set_events_open).into_any(),
        "rules" => render_rules(state, set_state, set_events_open).into_any(),
        "events" => render_events(state).into_any(),
        _ => render_metadata(state, set_state, set_events_open),
    }
}

/// Render metadata and category controls for active or selected items.
fn render_metadata(
    state: UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> AnyView {
    if !state.selected_ids.is_empty() {
        return view! {
            <div class="metadata-layout">
                {render_category_summary(state, set_state, set_events_open)}
            </div>
        }
        .into_any();
    }
    let fields = state.active_detail.as_ref().map(|detail| {
        let year = detail.year.map(|year| year.to_string()).unwrap_or_default();
        view! {
            <form method="post" action="/ui/action" class="metadata-form">
                <input type="hidden" name="return_to" value=state.return_to.clone()/>
                <input type="hidden" name="action" value="save_metadata"/>
                <input type="hidden" name="item_id" value=state.active_id.clone().unwrap_or_default()/>
                <input type="hidden" name="expected_revision" value=detail.metadata_revision.clone()/>
                {field("Title", "title", detail.title.clone())}
                {field("Authors", "authors", detail.authors.clone())}
                {field("Type", "item_type", detail.item_type.clone())}
                {field("Year", "year", year)}
                {field("DOI", "doi", detail.doi.clone().unwrap_or_default())}
                {field("Venue", "venue", detail.venue.clone().unwrap_or_default())}
                {field("Language", "language", detail.language.clone().unwrap_or_default())}
                {field("URI", "uri", detail.uri.clone().unwrap_or_default())}
                <label class="field wide">
                    "Abstract"
                    <textarea name="abstract_note">{detail.abstract_note.clone().unwrap_or_default()}</textarea>
                </label>
                <button class="button primary wide" type="submit">"Save Metadata"</button>
            </form>
        }
        .into_any()
    });
    view! {
        <div class="metadata-layout">
            {render_category_summary(state.clone(), set_state, set_events_open)}
            {fields.unwrap_or_else(|| empty_metadata_form(&state).into_any())}
        </div>
    }
    .into_any()
}

/// Render the rules editor.
fn render_rules(
    state: UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView {
    view! {
        <form
            method="post"
            action="/ui/action"
            class="rules-form"
            data-route-action="true"
            on:submit=move |event| {
                event.prevent_default();
                submit_action(event, set_state, set_events_open);
            }
        >
            <input type="hidden" name="return_to" value=state.return_to.clone()/>
            <input type="hidden" name="action" value="save_rules"/>
            <label class="field wide">
                <span>"Automatic Classification Rules"</span>
                <textarea name="rules_text">{state.rules_text.clone()}</textarea>
            </label>
            <button class="button primary" type="submit">"Save Rules"</button>
        </form>
    }
}

/// Render local file actions and file rows.
fn render_files(
    state: &UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView {
    let item_id = state.active_id.clone().unwrap_or_default();
    let return_to = state.return_to.clone();
    let upload_item_id = item_id.clone();
    let upload_return_to = return_to.clone();
    let picker_item_id = item_id.clone();
    let picker_return_to = return_to.clone();
    view! {
        <div class="files-pane">
            <section class="file-actions">
                <form method="post" action="/ui/action">
                    <input type="hidden" name="return_to" value=state.return_to.clone()/>
                    <input type="hidden" name="action" value="open_folder"/>
                    <input type="hidden" name="item_id" value=item_id.clone()/>
                    <button class="button secondary" type="submit">"Open Folder"</button>
                </form>
                <form
                    method="post"
                    action="/ui/upload"
                    enctype="multipart/form-data"
                    class="upload-zone"
                    on:dragover=move |event| {
                        event.prevent_default();
                    }
                    on:drop=move |event| {
                        event.prevent_default();
                        upload_dropped_files(
                            event,
                            upload_item_id.clone(),
                            upload_return_to.clone(),
                            set_state,
                            set_events_open,
                        );
                    }
                >
                    <input type="hidden" name="return_to" value=state.return_to.clone()/>
                    <input type="hidden" name="item_id" value=state.active_id.clone().unwrap_or_default()/>
                    <input
                        id="item-file-picker"
                        class="file-input"
                        type="file"
                        name="file"
                        multiple
                        on:change=move |event| {
                            upload_input_files(
                                event,
                                picker_item_id.clone(),
                                picker_return_to.clone(),
                                set_state,
                                set_events_open,
                            );
                        }
                    />
                    <label class="button primary" for="item-file-picker">"Add Files"</label>
                    <span>"Drop files here"</span>
                </form>
            </section>
            <div class="file-list">
                {state.files.iter().map(|file| render_file_row(state, file)).collect::<Vec<_>>()}
            </div>
        </div>
    }
}

/// Render one file row.
fn render_file_row(state: &UiState, file: &FileEntry) -> impl IntoView {
    let revision = state
        .active_detail
        .as_ref()
        .map(|detail| detail.metadata_revision.clone())
        .unwrap_or_default();
    view! {
        <div class="file-row">
            <span class="file-name">
                {file.path.clone()}
                {if file.is_main {
                    view! { <strong class="main-file-badge">"Main"</strong> }.into_any()
                } else {
                    view! {}.into_any()
                }}
            </span>
            <span>{format_file_size(file)}</span>
            {if file.is_main || !is_main_file_candidate(file) {
                view! {}.into_any()
            } else {
                view! {
                    <form method="post" action="/ui/action">
                        <input type="hidden" name="return_to" value=state.return_to.clone()/>
                        <input type="hidden" name="action" value="set_main_file"/>
                        <input type="hidden" name="item_id" value=state.active_id.clone().unwrap_or_default()/>
                        <input type="hidden" name="file_path" value=file.path.clone()/>
                        <input type="hidden" name="expected_revision" value=revision.clone()/>
                        <button class="button compact" type="submit">"Set Main"</button>
                    </form>
                }.into_any()
            }}
            <form method="post" action="/ui/action">
                <input type="hidden" name="return_to" value=state.return_to.clone()/>
                <input type="hidden" name="action" value="open_file"/>
                <input type="hidden" name="item_id" value=state.active_id.clone().unwrap_or_default()/>
                <input type="hidden" name="file_path" value=file.path.clone()/>
                <button class="button compact" type="submit">"Open"</button>
            </form>
        </div>
    }
}

/// Render category tags and transfer controls.
fn render_category_summary(
    state: UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView {
    let current = current_categories(&state);
    let available = available_categories(&state.categories, &current)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let target_ids = state.category_target_ids.clone();
    view! {
        <section class="category-summary">
            <div class="summary-head">
                <strong>"Current Categories:"</strong>
                <div class="tag-strip">
                    {current.iter().map(|path| view! {
                        <span class="category-tag">{path.clone()}</span>
                    }).collect::<Vec<_>>()}
                </div>
            </div>
            <details class="category-editor">
                <summary>"Edit Categories"</summary>
                <form
                    method="post"
                    action="/ui/action"
                    class="new-category"
                    data-route-action="true"
                    on:submit=move |event| {
                        event.prevent_default();
                        submit_action(event, set_state, set_events_open);
                    }
                >
                    <input type="hidden" name="return_to" value=state.return_to.clone()/>
                    <input type="hidden" name="action" value="create_category"/>
                    {target_ids.iter().map(|id| view! {
                        <input type="hidden" name="item_id" value=id.clone()/>
                    }).collect::<Vec<_>>()}
                    <label class="field">"New Category"<input name="category"/></label>
                    <button class="button primary" type="submit">"Create Category"</button>
                </form>
                <div class="transfer-grid">
                    <section>
                        <h4>"Current"</h4>
                        <div class="category-list">
                            {current.clone().into_iter().map(|path| {
                                render_category_action_row(
                                    state.clone(),
                                    path,
                                    "remove_category",
                                    "Remove",
                                    set_state,
                                    set_events_open,
                                )
                            }).collect::<Vec<_>>()}
                        </div>
                    </section>
                    <section>
                        <h4>"Available"</h4>
                        <div class="category-list">
                            {available.clone().into_iter().map(|category| {
                                render_category_action_row(
                                    state.clone(),
                                    category.path,
                                    "add_category",
                                    "Add",
                                    set_state,
                                    set_events_open,
                                )
                            }).collect::<Vec<_>>()}
                        </div>
                    </section>
                </div>
            </details>
        </section>
    }
}

/// Render one category add/remove row.
fn render_category_action_row(
    state: UiState,
    path: String,
    action: &'static str,
    label: &'static str,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView {
    view! {
        <form
            method="post"
            action="/ui/action"
            class="category-row"
            data-route-action="true"
            on:submit=move |event| {
                event.prevent_default();
                submit_action(event, set_state, set_events_open);
            }
        >
            <input type="hidden" name="return_to" value=state.return_to.clone()/>
            <input type="hidden" name="action" value=action/>
            <input type="hidden" name="category" value=path.clone()/>
            {state.category_target_ids.iter().map(|id| view! {
                <input type="hidden" name="item_id" value=id.clone()/>
            }).collect::<Vec<_>>()}
            <span>{path}</span>
            <button class="button tiny" type="submit">{label}</button>
        </form>
    }
}

/// Render recent daemon events.
fn render_events(state: UiState) -> impl IntoView {
    view! {
        <div class="event-list">
            {state.events.iter().rev().take(50).map(|event| view! {
                <div class="event-row">
                    <span>{event.id}</span>
                    <span>{event.kind.clone()}</span>
                    <strong>{event.message.clone()}</strong>
                </div>
            }).collect::<Vec<_>>()}
        </div>
    }
}

/// Render the floating rules save result dialog when present.
fn render_rules_notice(
    state: UiState,
    set_state: WriteSignal<UiState>,
) -> AnyView {
    match state.rules_notice {
        Some(RulesNotice::Saved { rules }) => view! {
            <aside class="rules-result-dialog" role="status">
                <div class="dialog-head">
                    <h3>"Parsed Rules"</h3>
                    <button
                        class="button compact"
                        type="button"
                        on:click=move |event| {
                            event.prevent_default();
                            dismiss_rules_notice(set_state);
                        }
                    >
                        "Close"
                    </button>
                </div>
                <div class="rules-result-list">
                    {rules.into_iter().map(|rule| view! {
                        <div class="rules-result-row">
                            <strong>{rule.name}</strong>
                            <span>{rule.target}</span>
                            <code>{rule.query}</code>
                        </div>
                    }).collect::<Vec<_>>()}
                </div>
            </aside>
        }
        .into_any(),
        Some(RulesNotice::Error { message }) => view! {
            <aside class="rules-result-dialog is-error" role="alert">
                <div class="dialog-head">
                    <h3>"Rules Error"</h3>
                    <button
                        class="button compact"
                        type="button"
                        on:click=move |event| {
                            event.prevent_default();
                            dismiss_rules_notice(set_state);
                        }
                    >
                        "Close"
                    </button>
                </div>
                <p>{message}</p>
            </aside>
        }
        .into_any(),
        None => view! {}.into_any(),
    }
}

/// Return the right pane title for single-item or bulk selection mode.
fn detail_title(state: &UiState) -> String {
    if state.tab == "rules" {
        return "Rules Editor".to_string();
    }
    match state.selected_ids.len() {
        0 => state
            .active_detail
            .as_ref()
            .map(|item| item.title.clone())
            .or_else(|| {
                state
                    .active_id
                    .as_ref()
                    .and_then(|id| {
                        state.items.iter().find(|item| &item.id == id)
                    })
                    .map(|item| item.title.clone())
            })
            .unwrap_or_else(|| "No item selected".to_string()),
        1 => "Selected 1 item".to_string(),
        count => format!("Selected {count} items"),
    }
}

/// Return one metadata text input.
fn field(
    label: &'static str,
    name: &'static str,
    value: String,
) -> impl IntoView {
    view! {
        <label class="field">
            <span>{label}</span>
            <input name=name value=value/>
        </label>
    }
}

/// Render an empty metadata form when no active item exists.
fn empty_metadata_form(state: &UiState) -> impl IntoView {
    view! {
        <form method="post" action="/ui/action" class="metadata-form">
            <input type="hidden" name="return_to" value=state.return_to.clone()/>
            <input type="hidden" name="action" value="save_metadata"/>
            <input type="hidden" name="item_id" value=""/>
            <input type="hidden" name="expected_revision" value=""/>
            {field("Title", "title", String::new())}
            {field("Authors", "authors", String::new())}
            {field("Type", "item_type", String::new())}
            {field("Year", "year", String::new())}
            {field("DOI", "doi", String::new())}
            {field("Venue", "venue", String::new())}
            {field("Language", "language", String::new())}
            {field("URI", "uri", String::new())}
            <label class="field wide">"Abstract"<textarea name="abstract_note"></textarea></label>
            <button class="button primary wide" type="submit">"Save Metadata"</button>
        </form>
    }
}

/// Return categories common to the current active or selected items.
fn current_categories(state: &UiState) -> Vec<String> {
    let target_ids = if state.category_target_ids.is_empty() {
        state.active_id.iter().cloned().collect::<Vec<_>>()
    } else {
        state.category_target_ids.clone()
    };
    let mut common: Option<std::collections::BTreeSet<String>> = None;
    for id in target_ids {
        let Some(item) = state.items.iter().find(|item| item.id == id) else {
            continue;
        };
        let categories = item.categories.iter().cloned().collect();
        common = Some(match common {
            Some(current) => {
                current.intersection(&categories).cloned().collect()
            }
            None => categories,
        });
    }
    common.unwrap_or_default().into_iter().collect()
}

/// Return the second-line summary for one item row.
fn item_subtitle(item: &ItemSummary) -> String {
    let mut parts = vec![item.id.clone(), item.item_type.clone()];
    if let Some(extension) = item.main_file.as_deref().and_then(file_extension)
    {
        parts.push(format!("main {extension}"));
    }
    if !item.categories.is_empty() {
        parts.push(item.categories.join(", "));
    }
    parts.join(" / ")
}

/// Return a display extension such as `.pdf` for a relative file path.
fn file_extension(path: &str) -> Option<String> {
    path.rsplit_once('.')
        .map(|(_, extension)| extension.trim())
        .filter(|extension| !extension.is_empty())
        .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
}

/// Return categories that are not already common to all selected items.
fn available_categories<'a>(
    categories: &'a [CategorySummary],
    current: &[String],
) -> Vec<&'a CategorySummary> {
    categories
        .iter()
        .filter(|category| !current.contains(&category.path))
        .collect()
}

/// Format a file size for display.
fn format_file_size(file: &FileEntry) -> String {
    match file.bytes {
        Some(bytes) if bytes < 1024 => format!("{bytes} B"),
        Some(bytes) if bytes < 1024 * 1024 => format!("{} KB", bytes / 1024),
        Some(bytes) => format!("{} MB", bytes / (1024 * 1024)),
        None => file.kind.clone(),
    }
}

/// Return whether a listed file can become the metadata main file.
fn is_main_file_candidate(file: &FileEntry) -> bool {
    file.kind == "file" && file.path != "metadata.toml"
}

/// Upload files selected from the hidden file input in hydrated browsers.
fn upload_input_files(
    event: leptos::ev::Event,
    item_id: String,
    return_to: String,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) {
    #[cfg(feature = "hydrate")]
    crate::client::upload_input_files(
        event,
        item_id,
        return_to,
        set_state,
        set_events_open,
    );

    #[cfg(not(feature = "hydrate"))]
    let _ = (event, item_id, return_to, set_state, set_events_open);
}

/// Upload files dropped on the file tab in hydrated browsers.
fn upload_dropped_files(
    event: leptos::ev::DragEvent,
    item_id: String,
    return_to: String,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) {
    #[cfg(feature = "hydrate")]
    crate::client::upload_dropped_files(
        event,
        item_id,
        return_to,
        set_state,
        set_events_open,
    );

    #[cfg(not(feature = "hydrate"))]
    let _ = (event, item_id, return_to, set_state, set_events_open);
}

/// Return the top navigation class for the events view.
fn top_event_class(active: bool) -> &'static str {
    if active { "top-link is-active" } else { "top-link" }
}

/// Return the right-pane tab class for route tabs.
fn right_tab_class(current: &str, tab: &str) -> &'static str {
    if current == tab { "right-tab is-active" } else { "right-tab" }
}

/// Return the visual class for the active watcher radio option.
fn watcher_class(active: bool) -> &'static str {
    if active { "radio-option is-active" } else { "radio-option" }
}

/// Visit a route in the hydrated browser; this is a no-op during SSR.
fn visit_route(
    route: RouteState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
    push_history: bool,
) {
    #[cfg(feature = "hydrate")]
    crate::client::visit_route(
        route,
        set_state,
        set_events_open,
        push_history,
    );

    #[cfg(not(feature = "hydrate"))]
    let _ = (route, set_state, set_events_open, push_history);
}

/// Submit a route action in the hydrated browser; this is a no-op during SSR.
fn submit_action(
    event: leptos::ev::SubmitEvent,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) {
    #[cfg(feature = "hydrate")]
    crate::client::submit_action(event, set_state, set_events_open);

    #[cfg(not(feature = "hydrate"))]
    let _ = (event, set_state, set_events_open);
}

/// Submit a form when a non-submit control changes in hydrated browsers.
fn submit_changed_form(
    event: leptos::ev::Event,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) {
    #[cfg(feature = "hydrate")]
    crate::client::submit_changed_form(event, set_state, set_events_open);

    #[cfg(not(feature = "hydrate"))]
    let _ = (event, set_state, set_events_open);
}

/// Dismiss the rules notice in hydrated state and URL.
fn dismiss_rules_notice(set_state: WriteSignal<UiState>) {
    set_state.update(|state| state.rules_notice = None);
    #[cfg(feature = "hydrate")]
    crate::client::clear_rules_notice_query();
}

#[cfg(test)]
mod tests {
    use super::document;
    use crate::model::UiState;
    use leptos::prelude::*;

    #[test]
    fn document_renders_hydratable_state_script() {
        let state = UiState {
            repo_name: "Localref".to_string(),
            search: None,
            category: None,
            items: Vec::new(),
            categories: Vec::new(),
            events: Vec::new(),
            pending_count: 0,
            selected_ids: Vec::new(),
            category_target_ids: Vec::new(),
            active_id: None,
            active_detail: None,
            tab: "metadata".to_string(),
            return_to: "/?tab=metadata".to_string(),
            status_label: "Running".to_string(),
            watcher_paused: false,
            files: Vec::new(),
            rules_text: String::new(),
            rules_notice: None,
        };

        let html = document(state).into_view().to_html();

        assert!(html.contains(r#"id="localref-ui-state""#));
        assert!(html.contains("Localref"));
    }

    #[test]
    fn item_subtitle_includes_main_file_extension() {
        let item = crate::model::ItemSummary {
            id: "lr:zotero:one".to_string(),
            title: "Paper".to_string(),
            authors: Vec::new(),
            item_type: "preprint".to_string(),
            categories: vec!["A".to_string()],
            main_file: Some("paper.PDF".to_string()),
            files: vec!["paper.PDF".to_string()],
        };

        assert_eq!(
            super::item_subtitle(&item),
            "lr:zotero:one / preprint / main .pdf / A"
        );
    }

    #[test]
    fn metadata_toml_is_not_a_main_file_candidate() {
        let metadata = crate::model::FileEntry {
            path: "metadata.toml".to_string(),
            kind: "file".to_string(),
            bytes: Some(10),
            is_main: false,
        };
        let attachment = crate::model::FileEntry {
            path: "notes.txt".to_string(),
            kind: "file".to_string(),
            bytes: Some(5),
            is_main: false,
        };

        assert!(!super::is_main_file_candidate(&metadata));
        assert!(super::is_main_file_candidate(&attachment));
    }
}
