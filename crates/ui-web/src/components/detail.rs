//! Detail-pane components for metadata, files, and events.

use leptos::prelude::*;

use crate::state::{UiModel, author_summary, field};

use super::category::render_category_summary;
use super::events::render_events;
use super::files::render_files;

/// Render the current right-pane content for the selected tab.
pub(super) fn render_detail(model: &UiModel) -> impl IntoView {
    if !model.selected_ids.is_empty() && model.tab != "rules" {
        return render_metadata(model).into_any();
    }
    match model.tab.as_str() {
        "files" => render_files(model).into_any(),
        "rules" => render_rules(model).into_any(),
        "events" => render_events(model).into_any(),
        _ => render_metadata(model).into_any(),
    }
}

/// Render the metadata editor with category management above the summary form.
fn render_metadata(model: &UiModel) -> impl IntoView {
    let metadata = model.active_metadata.as_ref();
    let metadata_form = if model.selected_ids.is_empty() {
        view! {
            <form method="post" action="/ui/action" class="metadata-form">
                <input type="hidden" name="return_to" value={model.return_to.clone()}/>
                <input type="hidden" name="action" value="save_metadata"/>
                <input type="hidden" name="item_id" value={model.active_id.clone().unwrap_or_default()}/>
                <input type="hidden" name="expected_revision" value={metadata.map(|doc| doc.metadata_revision.clone()).unwrap_or_default()}/>
                {field("Title", "title", metadata.map(|doc| doc.metadata.title.clone()).unwrap_or_default())}
                {field("Authors", "authors", metadata.map(|doc| author_summary(&doc.metadata)).unwrap_or_default())}
                {field("Type", "item_type", metadata.map(|doc| doc.metadata.item_type.clone()).unwrap_or_default())}
                {field("Year", "year", metadata.and_then(|doc| doc.metadata.year).map(|year| year.to_string()).unwrap_or_default())}
                {field("DOI", "doi", metadata.and_then(|doc| doc.metadata.doi.clone()).unwrap_or_default())}
                {field("Venue", "venue", metadata.and_then(|doc| doc.metadata.venue.clone()).unwrap_or_default())}
                {field("Language", "language", metadata.and_then(|doc| doc.metadata.language.clone()).unwrap_or_default())}
                {field("URI", "uri", metadata.and_then(|doc| doc.metadata.uri.clone()).unwrap_or_default())}
                <label class="field wide">"Abstract"<textarea name="abstract_note">{metadata.and_then(|doc| doc.metadata.abstract_note.clone()).unwrap_or_default()}</textarea></label>
                <button class="button primary wide" type="submit">"Save Metadata"</button>
            </form>
        }
        .into_any()
    } else {
        view! { <div></div> }.into_any()
    };
    view! {
        <div class="metadata-layout">
            {render_category_summary(model)}
            {metadata_form}
        </div>
    }
}

/// Render a text editor for automatic classification rules.
fn render_rules(model: &UiModel) -> impl IntoView {
    view! {
        <form method="post" action="/ui/action" class="rules-form" data-route-action="true">
            <input type="hidden" name="return_to" value={model.return_to.clone()}/>
            <input type="hidden" name="action" value="save_rules"/>
            <label class="field wide">
                <span>"Automatic Classification Rules"</span>
                <textarea name="rules_text">{model.rules_text.clone()}</textarea>
            </label>
            <button class="button primary" type="submit">"Save Rules"</button>
        </form>
    }
}
