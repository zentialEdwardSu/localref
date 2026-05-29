//! Sidebar components for library filtering and selection.

use leptos::prelude::*;

use crate::state::UiModel;

/// Render the left library pane with sticky filters and a scrollable item list.
pub(super) fn render_sidebar(
    model: &UiModel,
    _selected_count: usize,
) -> impl IntoView {
    view! {
        <aside class="library-pane">
            <div class="filter-panel">
                <label class="field">
                    <span>"Search"</span>
                    <input id="library-search" name="q" value={model.query.q.clone().unwrap_or_default()}/>
                </label>
                <label class="field">
                    <span>"Category Filter"</span>
                    <select id="library-category" name="category">
                        <option value="">"All"</option>
                        {model.categories.iter().map(|category| {
                            let selected = model.query.category.as_deref() == Some(category.path.as_str());
                            view! {
                                <option value={category.path.clone()} selected=selected>{category.path.clone()}</option>
                            }
                        }).collect::<Vec<_>>()}
                    </select>
                </label>
            </div>
            <form method="get" action="/" class="selection-form">
                <input data-filter-q="true" type="hidden" name="q" value={model.query.q.clone().unwrap_or_default()}/>
                <input data-filter-category="true" type="hidden" name="category" value={model.query.category.clone().unwrap_or_default()}/>
                <input type="hidden" name="active" value={model.active_id.clone().unwrap_or_default()}/>
                <input type="hidden" name="tab" value={model.tab.clone()}/>
                <div id="library-list" class="item-list">
                    {model.items.iter().map(|item| {
                        let checked = model.selected_ids.contains(&item.id);
                        let route_tab = if model.tab == "files" { "files" } else { "metadata" };
                        let open_file = openable_file(item);
                        let row_class = if model.active_id.as_deref() == Some(item.id.as_str()) {
                            "item-row is-active"
                        } else {
                            "item-row"
                        };
                        view! {
                            <div class=row_class data-title={item.title.to_lowercase()} data-id={item.id.to_lowercase()} data-authors={item.authors.join(" ").to_lowercase()} data-categories={item.categories.join("|")}>
                                <input class="row-check" type="checkbox" name="item" value={item.id.clone()} checked=checked/>
                                <button class="item-link" type="button" data-route-active={item.id.clone()} data-route-tab=route_tab data-open-file=open_file>
                                    <strong>{item.title.clone()}</strong>
                                    <span>{item.id.clone()} " / " {item.item_type.clone()}</span>
                                </button>
                            </div>
                        }
                    }).collect::<Vec<_>>()}
                </div>
            </form>
        </aside>
    }
}

/// Return the best file to open directly when an item is double-clicked.
fn openable_file(item: &localref_core::model::ItemDocument) -> String {
    item.main_file
        .iter()
        .chain(item.extra_files.iter())
        .find(|path| is_openable_file(path))
        .cloned()
        .unwrap_or_default()
}

fn is_openable_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".pdf")
        || lower.ends_with(".epub")
        || lower.ends_with(".html")
        || lower.ends_with(".htm")
        || lower.ends_with(".url")
}
