//! HTML rendering helpers for browser-updated Localref UI regions.

use crate::api::{FileEntryDto, UiStateDto};

/// Render the library item list from JSON state.
pub fn render_item_list_html(state: &UiStateDto) -> String {
    state
        .items
        .iter()
        .map(|item| {
            let row_class =
                if state.active_id.as_deref() == Some(item.id.as_str()) {
                    "item-row is-active"
                } else {
                    "item-row"
                };
            let checked = if state.selected_ids.contains(&item.id) {
                " checked"
            } else {
                ""
            };
            let route_tab =
                if state.tab == "files" { "files" } else { "metadata" };
            format!(
                r#"<div class="{row_class}" data-title="{title_l}" data-id="{id_l}" data-authors="{authors_l}" data-categories="{categories}"><input class="row-check" type="checkbox" name="item" value="{id}"{checked}/><button class="item-link" type="button" data-route-active="{id}" data-route-tab="{route_tab}" data-open-file="{open_file}"><strong>{title}</strong><span>{id} / {item_type}</span></button></div>"#,
                row_class = row_class,
                title_l = escape_attr(&item.title.to_ascii_lowercase()),
                id_l = escape_attr(&item.id.to_ascii_lowercase()),
                authors_l =
                    escape_attr(&item.authors.join(" ").to_ascii_lowercase()),
                categories = escape_attr(&item.categories.join("|")),
                id = escape_attr(&item.id),
                checked = checked,
                route_tab = route_tab,
                open_file =
                    escape_attr(item.main_file.as_deref().unwrap_or_default()),
                title = escape_text(&item.title),
                item_type = escape_text(&item.item_type),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Render the right-pane title and tab controls from JSON state.
pub fn render_detail_head_html(state: &UiStateDto) -> String {
    let item_tabs = if state.selected_ids.is_empty() {
        format!(
            r#"<button class="{metadata}" type="button" data-route-tab="metadata">Metadata</button><button class="{files}" type="button" data-route-tab="files">Files</button>"#,
            metadata = right_tab_class(&state.tab, "metadata"),
            files = right_tab_class(&state.tab, "files"),
        )
    } else {
        String::new()
    };
    format!(
        r#"<div class="title-block"><h2>{title}</h2></div><nav class="right-tabs">{item_tabs}<button class="{rules}" type="button" data-route-tab="rules">Rules</button></nav>"#,
        title = escape_text(&detail_title(state)),
        rules = right_tab_class(&state.tab, "rules"),
    )
}

/// Render the right-pane detail body from JSON state.
pub fn render_detail_html(state: &UiStateDto) -> String {
    if !state.selected_ids.is_empty() && state.tab != "rules" {
        return render_metadata_html(state);
    }
    match state.tab.as_str() {
        "files" => render_files_html(state),
        "rules" => render_rules_html(state),
        "events" => render_events_html(state),
        _ => render_metadata_html(state),
    }
}

/// Render the event panel body from JSON state.
pub fn render_events_html(state: &UiStateDto) -> String {
    let rows = state
        .events
        .iter()
        .rev()
        .take(50)
        .map(|event| {
            format!(
                r#"<div class="event-row"><span>{}</span><span>{}</span><strong>{}</strong></div>"#,
                event.id,
                escape_text(&event.kind),
                escape_text(&event.message)
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(r#"<div class="event-list">{rows}</div>"#)
}

/// Return the detail title matching the server-rendered title rules.
fn detail_title(state: &UiStateDto) -> String {
    if state.tab == "rules" {
        return "Rules Editor".to_string();
    }
    match state.selected_ids.len() {
        0 => state
            .active_detail
            .as_ref()
            .map(|detail| detail.title.clone())
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

/// Return the class for a right-pane tab button.
fn right_tab_class(current: &str, tab: &str) -> &'static str {
    if current == tab { "right-tab is-active" } else { "right-tab" }
}

/// Render local file actions and file rows from JSON state.
fn render_files_html(state: &UiStateDto) -> String {
    let rows = state
        .files
        .iter()
        .map(|file| render_file_row(state, file))
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<div class="files-pane"><section class="file-actions"><form method="post" action="/ui/action"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="open_folder"/><input type="hidden" name="item_id" value="{active_id}"/><button class="button secondary" type="submit">Open Folder</button></form><form method="post" action="/ui/action" class="path-form"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="add_file"/><input type="hidden" name="item_id" value="{active_id}"/><label class="field">Local File Path<input name="file_path"/></label><button class="button primary" type="submit">Add File</button></form><form method="post" action="/ui/action" class="path-form"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="import_file"/><label class="field">Import Path<input name="file_path"/></label><button class="button secondary" type="submit">Import File</button></form></section><div class="file-list">{rows}</div></div>"#,
        return_to = escape_attr(&state.return_to),
        active_id =
            escape_attr(state.active_id.as_deref().unwrap_or_default()),
    )
}

/// Render one file row with its open action.
fn render_file_row(state: &UiStateDto, file: &FileEntryDto) -> String {
    format!(
        r#"<div class="file-row"><span>{}</span><span>{}</span><form method="post" action="/ui/action"><input type="hidden" name="return_to" value="{}"/><input type="hidden" name="action" value="open_file"/><input type="hidden" name="item_id" value="{}"/><input type="hidden" name="file_path" value="{}"/><button class="button compact" type="submit">Open</button></form></div>"#,
        escape_text(&file.path),
        escape_text(&format_file_size(file)),
        escape_attr(&state.return_to),
        escape_attr(state.active_id.as_deref().unwrap_or_default()),
        escape_attr(&file.path),
    )
}

/// Format a file size the same way as the server-rendered file component.
fn format_file_size(file: &FileEntryDto) -> String {
    match file.bytes {
        Some(bytes) if bytes < 1024 => format!("{bytes} B"),
        Some(bytes) if bytes < 1024 * 1024 => format!("{} KB", bytes / 1024),
        Some(bytes) => format!("{} MB", bytes / (1024 * 1024)),
        None => file.kind.clone(),
    }
}

/// Render the automatic-classification rules editor.
fn render_rules_html(state: &UiStateDto) -> String {
    format!(
        r#"<form method="post" action="/ui/action" class="rules-form" data-route-action="true"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="save_rules"/><label class="field wide"><span>Automatic Classification Rules</span><textarea name="rules_text">{rules_text}</textarea></label><button class="button primary" type="submit">Save Rules</button></form>"#,
        return_to = escape_attr(&state.return_to),
        rules_text = escape_text(&state.rules_text),
    )
}

/// Render metadata and category controls for active or selected items.
fn render_metadata_html(state: &UiStateDto) -> String {
    let category_summary = render_category_summary_html(state);
    if !state.selected_ids.is_empty() {
        return format!(
            r#"<div class="metadata-layout">{category_summary}</div>"#
        );
    }
    let Some(detail) = &state.active_detail else {
        return format!(
            r#"<div class="metadata-layout">{category_summary}{}</div>"#,
            render_empty_metadata_form(state)
        );
    };
    let year = detail.year.map(|year| year.to_string()).unwrap_or_default();
    let fields = [
        render_field("Title", "title", &detail.title),
        render_field("Authors", "authors", &detail.authors),
        render_field("Type", "item_type", &detail.item_type),
        render_field("Year", "year", &year),
        render_field("DOI", "doi", detail.doi.as_deref().unwrap_or_default()),
        render_field(
            "Venue",
            "venue",
            detail.venue.as_deref().unwrap_or_default(),
        ),
        render_field(
            "Language",
            "language",
            detail.language.as_deref().unwrap_or_default(),
        ),
        render_field("URI", "uri", detail.uri.as_deref().unwrap_or_default()),
    ]
    .join("");
    format!(
        r#"<div class="metadata-layout">{category_summary}<form method="post" action="/ui/action" class="metadata-form"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="save_metadata"/><input type="hidden" name="item_id" value="{item_id}"/><input type="hidden" name="expected_revision" value="{revision}"/>{fields}<label class="field wide">Abstract<textarea name="abstract_note">{abstract_note}</textarea></label><button class="button primary wide" type="submit">Save Metadata</button></form></div>"#,
        category_summary = category_summary,
        return_to = escape_attr(&state.return_to),
        item_id = escape_attr(state.active_id.as_deref().unwrap_or_default()),
        revision = escape_attr(&detail.metadata_revision),
        fields = fields,
        abstract_note =
            escape_text(detail.abstract_note.as_deref().unwrap_or_default()),
    )
}

/// Render one metadata form field.
fn render_field(label: &str, name: &str, value: &str) -> String {
    format!(
        r#"<label class="field"><span>{}</span><input name="{}" value="{}"/></label>"#,
        escape_text(label),
        escape_attr(name),
        escape_attr(value)
    )
}

/// Render an empty metadata form when no active item exists.
fn render_empty_metadata_form(state: &UiStateDto) -> String {
    format!(
        r#"<form method="post" action="/ui/action" class="metadata-form"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="save_metadata"/><input type="hidden" name="item_id" value=""/><input type="hidden" name="expected_revision" value=""/><label class="field"><span>Title</span><input name="title" value=""/></label><label class="field"><span>Authors</span><input name="authors" value=""/></label><label class="field"><span>Type</span><input name="item_type" value=""/></label><label class="field"><span>Year</span><input name="year" value=""/></label><label class="field"><span>DOI</span><input name="doi" value=""/></label><label class="field"><span>Venue</span><input name="venue" value=""/></label><label class="field"><span>Language</span><input name="language" value=""/></label><label class="field"><span>URI</span><input name="uri" value=""/></label><label class="field wide">Abstract<textarea name="abstract_note"></textarea></label><button class="button primary wide" type="submit">Save Metadata</button></form>"#,
        return_to = escape_attr(&state.return_to),
    )
}

/// Render category tags and transfer controls.
fn render_category_summary_html(state: &UiStateDto) -> String {
    let current = current_categories(state);
    let current_tags = current
        .iter()
        .map(|category| {
            format!(
                r#"<span class="category-tag">{}</span>"#,
                escape_text(category)
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let available_rows = state
        .categories
        .iter()
        .filter(|category| !current.contains(&category.path))
        .map(|category| {
            let path = escape_attr(&category.path);
            let label = escape_text(&category.path);
            format!(
                r#"<form method="post" action="/ui/action" class="category-row" data-route-action="true"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="add_category"/><input type="hidden" name="category" value="{path}"/><span>{label}</span><button class="button tiny" type="submit">Add</button></form>"#,
                return_to = escape_attr(&state.return_to),
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let current_rows = current
        .iter()
        .map(|category| {
            let path = escape_attr(category);
            let label = escape_text(category);
            format!(
                r#"<form method="post" action="/ui/action" class="category-row" data-route-action="true"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="remove_category"/><input type="hidden" name="category" value="{path}"/><span>{label}</span><button class="button tiny" type="submit">Remove</button></form>"#,
                return_to = escape_attr(&state.return_to),
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<section class="category-summary"><details class="category-editor"><summary>Current Categories: <div class="tag-strip">{current_tags}</div></summary><form method="post" action="/ui/action" class="new-category" data-route-action="true"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="create_category"/><input name="category" placeholder="Category path"/><button class="button secondary" type="submit">Create Category</button></form><div class="transfer-grid"><section><h4>Available</h4><div class="category-list">{available_rows}</div></section><section><h4>Current</h4><div class="category-list">{current_rows}</div></section></div></details></section>"#,
        return_to = escape_attr(&state.return_to),
    )
}

/// Return categories common to the current active or selected items.
fn current_categories(state: &UiStateDto) -> Vec<String> {
    let target_ids = if state.category_target_ids.is_empty() {
        state.active_id.iter().cloned().collect::<Vec<_>>()
    } else {
        state.category_target_ids.clone()
    };
    let mut common: Option<Vec<String>> = None;
    for id in target_ids {
        let Some(item) = state.items.iter().find(|item| item.id == id) else {
            continue;
        };
        common = Some(match common {
            Some(current) => current
                .into_iter()
                .filter(|category| item.categories.contains(category))
                .collect(),
            None => item.categories.clone(),
        });
    }
    common.unwrap_or_default()
}

/// Escape text for an HTML text node.
fn escape_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Escape text for an HTML attribute.
fn escape_attr(value: &str) -> String {
    escape_text(value).replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::{
        render_detail_head_html, render_detail_html, render_events_html,
        render_item_list_html,
    };
    use crate::api::UiStateDto;

    #[test]
    fn item_list_marks_active_and_checked_rows() {
        let mut state = state_fixture("metadata");
        state.selected_ids.push("lr:zotero:alpha".to_string());

        let html = render_item_list_html(&state);

        assert!(html.contains("item-row is-active"));
        assert!(html.contains(r#"checked"#));
        assert!(html.contains("Alpha &amp; Beta"));
        assert!(html.contains(r#"data-route-active="lr:zotero:alpha""#));
    }

    #[test]
    fn detail_html_renders_files_tab_from_json_state() {
        let state = state_fixture("files");

        let html = render_detail_html(&state);

        assert!(html.contains("Open Folder"));
        assert!(html.contains("Add File"));
        assert!(html.contains("Import File"));
        assert!(html.contains("paper.pdf"));
        assert!(html.contains("12 B"));
        assert!(!html.contains("Save Metadata"));
    }

    #[test]
    fn detail_html_renders_rules_editor_from_json_state() {
        let state = state_fixture("rules");

        let head = render_detail_head_html(&state);
        let html = render_detail_html(&state);

        assert!(head.contains("Rules Editor"));
        assert!(head.contains("right-tab is-active"));
        assert!(html.contains("Automatic Classification Rules"));
        assert!(html.contains(r#"name="rules_text""#));
        assert!(html.contains("[[rules]]"));
        assert!(!html.contains("Save Metadata"));
    }

    #[test]
    fn detail_html_renders_full_metadata_form_from_json_state() {
        let state = state_fixture("metadata");

        let html = render_detail_html(&state);

        for label in [
            "Title", "Authors", "Type", "Year", "DOI", "Venue", "Language",
            "URI", "Abstract",
        ] {
            assert!(html.contains(label), "missing metadata label {label}");
        }
        assert!(html.contains("Save Metadata"));
    }

    #[test]
    fn events_html_renders_recent_events_from_json_state() {
        let state = state_fixture("events");

        let html = render_events_html(&state);

        assert!(html.contains("ImportFinished"));
        assert!(html.contains("connector import finished"));
    }

    #[test]
    fn detail_html_keeps_category_editor_without_active_item() {
        let state: UiStateDto = serde_json::from_str(
            r#"{
                "items": [],
                "categories": [{"path": "Inbox", "item_count": 0}],
                "events": [],
                "pending_count": 0,
                "selected_ids": [],
                "category_target_ids": [],
                "active_id": null,
                "active_detail": null,
                "tab": "metadata",
                "return_to": "/?tab=metadata",
                "status_label": "Running",
                "files": [],
                "rules_text": "",
                "rules_notice": null
            }"#,
        )
        .unwrap();

        let html = render_detail_html(&state);

        assert!(html.contains("Current Categories:"));
        assert!(html.contains("Create Category"));
        assert!(html.contains(r#"data-route-action="true""#));
        assert!(html.contains("Save Metadata"));
    }

    fn state_fixture(tab: &str) -> UiStateDto {
        serde_json::from_str(&format!(
            r#"{{
                "items": [{{"id": "lr:zotero:alpha", "title": "Alpha & Beta", "authors": ["Ada"], "item_type": "journalArticle", "categories": ["Inbox"], "main_file": "paper.pdf"}}],
                "categories": [{{"path": "Inbox", "item_count": 1}}],
                "events": [{{"id": 1, "kind": "ImportFinished", "message": "connector import finished", "item_id": "lr:zotero:alpha", "path": null}}],
                "pending_count": 0,
                "selected_ids": [],
                "category_target_ids": ["lr:zotero:alpha"],
                "active_id": "lr:zotero:alpha",
                "active_detail": {{"metadata_revision": "rev1", "title": "Alpha & Beta", "authors": "Ada", "item_type": "journalArticle", "year": 2026, "doi": "10.1/alpha", "venue": "Journal", "language": "en", "uri": "https://example.test", "abstract_note": "Abstract"}},
                "tab": "{tab}",
                "return_to": "/?active=lr:zotero:alpha&tab={tab}",
                "status_label": "Running",
                "files": [{{"path": "paper.pdf", "kind": "file", "bytes": 12}}],
                "rules_text": "[[rules]]\nname = \"Inbox\"\ntarget = \"Inbox\"\nquery = 'title:alpha'\n",
                "rules_notice": null
            }}"#,
        ))
        .unwrap()
    }
}
