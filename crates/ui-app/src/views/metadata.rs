//! Metadata view, edit form, and category management.

use leptos::prelude::*;

use crate::model::{CategorySummary, UiState};
use crate::ui::badge::{Badge, BadgeVariant};
use crate::ui::button::{Button, ButtonVariant, ButtonSize};
use crate::ui::card::*;

/// Render metadata + categories for active or selected items.
pub fn render_metadata(
    state: &UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> AnyView {
    let error_view = state.plugin_error.clone().map(|msg| view! {
        <div class="plugin-error" role="alert"><h3>"Plugin error"</h3><p>{msg}</p></div>
    });

    if !state.selected_ids.is_empty() {
        let category_view = render_category_summary(state, set_state, set_events_open);
        let plugin_slots = super::plugins::render_plugin_slots(state, "selection_page");
        return view! {
            <div class="p-4 grid gap-4">
                {error_view}
                {category_view}
                {plugin_slots}
            </div>
        }.into_any();
    }

    let category_view = render_category_summary(state, set_state, set_events_open);
    let plugin_slots = super::plugins::render_plugin_slots(state, "metadata_page");

    let fields = state.active_detail.as_ref().map(|detail| {
        let return_to = state.return_to.clone();
        let active_id = state.active_id.clone().unwrap_or_default();
        let metadata_revision = detail.metadata_revision.clone();
        let title = detail.title.clone();
        let authors = detail.authors.clone();
        let item_type = detail.item_type.clone();
        let year = detail.year.map(|y| y.to_string()).unwrap_or_default();
        let doi = detail.doi.clone().unwrap_or_default();
        let venue = detail.venue.clone().unwrap_or_default();
        let language = detail.language.clone().unwrap_or_default();
        let uri = detail.uri.clone().unwrap_or_default();
        let abstract_note = detail.abstract_note.clone().unwrap_or_default();

        view! {
            <Card>
                <CardHeader>
                    <CardTitle>"Metadata"</CardTitle>
                </CardHeader>
                <CardContent>
                    <form method="post" action="/ui/action" class="grid gap-3 min-[900px]:grid-cols-2"
                        data-route-action="true"
                        on:submit=move |event| {
                            event.prevent_default();
                            super::submit_action(event, set_state, set_events_open);
                        }
                    >
                        <input type="hidden" name="return_to" value=return_to />
                        <input type="hidden" name="action" value="save_metadata" />
                        <input type="hidden" name="item_id" value=active_id />
                        <input type="hidden" name="expected_revision" value=metadata_revision />
                        {field("Title", "title", title)}
                        {field("Authors", "authors", authors)}
                        {field("Type", "item_type", item_type)}
                        {field("Year", "year", year)}
                        {field("DOI", "doi", doi)}
                        {field("Venue", "venue", venue)}
                        {field("Language", "language", language)}
                        {field("URI", "uri", uri)}
                        <label class="grid gap-1.5 text-sm font-medium col-span-full">
                            "Abstract"
                            <textarea
                                name="abstract_note"
                                class="flex min-h-24 w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/50 resize-y"
                            >{abstract_note}</textarea>
                        </label>
                        <div class="col-span-full">
                            <Button variant=ButtonVariant::Default size=ButtonSize::Default attr:r#type="submit">
                                "Save Metadata"
                            </Button>
                        </div>
                    </form>
                </CardContent>
            </Card>
        }.into_any()
    });

    view! {
        <div class="p-4 grid gap-4">
            {error_view}
            {category_view}
            {fields.unwrap_or_else(|| empty_metadata(state).into_any())}
            {plugin_slots}
        </div>
    }.into_any()
}

/// Render an empty metadata placeholder.
fn empty_metadata(state: &UiState) -> impl IntoView + use<> {
    let return_to = state.return_to.clone();

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Metadata"</CardTitle>
            </CardHeader>
            <CardContent>
                <form method="post" action="/ui/action" class="grid gap-3 min-[900px]:grid-cols-2">
                    <input type="hidden" name="return_to" value=return_to />
                    <input type="hidden" name="action" value="save_metadata" />
                    <input type="hidden" name="item_id" value="" />
                    <input type="hidden" name="expected_revision" value="" />
                    {field("Title", "title", String::new())}
                    {field("Authors", "authors", String::new())}
                    {field("Type", "item_type", String::new())}
                    {field("Year", "year", String::new())}
                    {field("DOI", "doi", String::new())}
                    {field("Venue", "venue", String::new())}
                    {field("Language", "language", String::new())}
                    {field("URI", "uri", String::new())}
                    <label class="grid gap-1.5 text-sm font-medium col-span-full">
                        "Abstract"
                        <textarea name="abstract_note"
                            class="flex min-h-24 w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/50 resize-y"
                        />
                    </label>
                    <div class="col-span-full">
                        <Button variant=ButtonVariant::Default size=ButtonSize::Default attr:r#type="submit">
                            "Save Metadata"
                        </Button>
                    </div>
                </form>
            </CardContent>
        </Card>
    }
}

/// One metadata text input.
fn field(label: &'static str, name: &'static str, value: String) -> impl IntoView {
    view! {
        <label class="grid gap-1.5 text-sm font-medium">
            <span>{label}</span>
            <input
                name=name
                value=value
                class="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/50"
            />
        </label>
    }
}

/// Category tags and transfer controls.
fn render_category_summary(
    state: &UiState,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView + use<> {
    let current = current_categories(state);
    let available: Vec<_> = available_categories(&state.categories, &current)
        .into_iter().cloned().collect();
    let target_ids = state.category_target_ids.clone();
    let return_to = state.return_to.clone();
    let category_target_ids = state.category_target_ids.clone();
    let category_target_ids2 = state.category_target_ids.clone();
    let return_to_remove = state.return_to.clone();
    let return_to_add = state.return_to.clone();

    // Clone current for use in the "Current" section (will be consumed by into_iter)
    let current_for_badges = current.clone();
    let current_for_remove = current.clone();

    view! {
        <Card>
            <CardContent class="pt-4">
                <div class="flex flex-wrap items-center gap-2 mb-3">
                    <span class="text-sm font-medium">"Categories:"</span>
                    {current_for_badges.into_iter().map(|path| view! {
                        <Badge variant=BadgeVariant::Secondary>{path}</Badge>
                    }).collect::<Vec<_>>()}
                </div>
                <details class="border border-border rounded-md">
                    <summary class="cursor-pointer px-3 py-2 text-sm font-medium hover:bg-muted">
                        "Edit Categories"
                    </summary>
                    <div class="border-t border-border p-3 grid gap-3">
                        // New category form
                        <form
                            method="post" action="/ui/action"
                            class="flex gap-2 items-end"
                            data-route-action="true"
                            on:submit=move |event| {
                                event.prevent_default();
                                super::submit_action(event, set_state, set_events_open);
                            }
                        >
                            <input type="hidden" name="return_to" value=return_to />
                            <input type="hidden" name="action" value="create_category" />
                            {target_ids.into_iter().map(|id| view! {
                                <input type="hidden" name="item_id" value=id />
                            }).collect::<Vec<_>>()}
                            <input
                                name="category"
                                placeholder="New category"
                                class="flex h-9 flex-1 rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/50"
                            />
                            <Button variant=ButtonVariant::Default size=ButtonSize::Sm attr:r#type="submit">
                                "Create"
                            </Button>
                        </form>
                        // Transfer grid
                        <div class="grid min-[900px]:grid-cols-2 gap-3">
                            <div>
                                <h4 class="text-sm font-medium mb-2">"Current"</h4>
                                {current_for_remove.into_iter().map(|path| {
                                    category_row_owned(category_target_ids.clone(), return_to_remove.clone(), path, "remove_category", "Remove", set_state, set_events_open)
                                }).collect::<Vec<_>>()}
                            </div>
                            <div>
                                <h4 class="text-sm font-medium mb-2">"Available"</h4>
                                {available.into_iter().map(|cat| {
                                    category_row_owned(category_target_ids2.clone(), return_to_add.clone(), cat.path, "add_category", "Add", set_state, set_events_open)
                                }).collect::<Vec<_>>()}
                            </div>
                        </div>
                    </div>
                </details>
            </CardContent>
        </Card>
    }
}

/// One category add/remove row — takes all owned data.
fn category_row_owned(
    category_target_ids: Vec<String>,
    return_to: String,
    path: String,
    action: &'static str,
    label: &'static str,
    set_state: WriteSignal<UiState>,
    set_events_open: WriteSignal<bool>,
) -> impl IntoView {
    let target_ids = category_target_ids;

    view! {
        <form
            method="post" action="/ui/action"
            class="flex items-center justify-between py-1.5 px-2 rounded hover:bg-muted"
            data-route-action="true"
            on:submit=move |event| {
                event.prevent_default();
                super::submit_action(event, set_state, set_events_open);
            }
        >
            <input type="hidden" name="return_to" value=return_to />
            <input type="hidden" name="action" value=action />
            <input type="hidden" name="category" value=path.clone() />
            {target_ids.into_iter().map(|id| view! {
                <input type="hidden" name="item_id" value=id />
            }).collect::<Vec<_>>()}
            <span class="text-sm truncate">{path}</span>
            <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm attr:r#type="submit">
                {label}
            </Button>
        </form>
    }
}

/// Categories common to active/selected items.
fn current_categories(state: &UiState) -> Vec<String> {
    let target_ids: &[String] = if state.category_target_ids.is_empty() {
        state.active_id.as_slice()
    } else {
        &state.category_target_ids
    };
    let common = target_ids
        .iter()
        .filter_map(|id| state.items.iter().find(|item| &item.id == id))
        .fold(None, |common: Option<std::collections::BTreeSet<String>>, item| {
            let categories = item.categories.iter().cloned().collect();
            Some(match common {
                Some(cur) => cur.intersection(&categories).cloned().collect(),
                None => categories,
            })
        });
    common.unwrap_or_default().into_iter().collect()
}

/// Categories not already in current set.
fn available_categories<'a>(
    categories: &'a [CategorySummary],
    current: &[String],
) -> Vec<&'a CategorySummary> {
    categories.iter().filter(|c| !current.contains(&c.path)).collect()
}
