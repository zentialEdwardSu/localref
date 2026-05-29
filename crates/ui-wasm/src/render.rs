//! HTML rendering helpers for browser-updated Localref UI regions.

use crate::api::UiStateDto;

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
            let route_tab = if state.tab == "files" { "files" } else { "metadata" };
            format!(
                r#"<div class="{row_class}" data-title="{title_l}" data-id="{id_l}" data-authors="{authors_l}" data-categories="{categories}"><input class="row-check" type="checkbox" name="item" value="{id}"{checked}/><button class="item-link" type="button" data-route-active="{id}" data-route-tab="{route_tab}" data-open-file="{open_file}"><strong>{title}</strong><span>{id} / {item_type}</span></button></div>"#,
                row_class = row_class,
                title_l = escape_attr(&item.title.to_ascii_lowercase()),
                id_l = escape_attr(&item.id.to_ascii_lowercase()),
                authors_l = escape_attr(&item.authors.join(" ").to_ascii_lowercase()),
                categories = escape_attr(&item.categories.join("|")),
                id = escape_attr(&item.id),
                checked = checked,
                route_tab = route_tab,
                open_file = escape_attr(item.main_file.as_deref().unwrap_or_default()),
                title = escape_text(&item.title),
                item_type = escape_text(&item.item_type),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Render the right-pane detail body from JSON state.
pub fn render_detail_html(state: &UiStateDto) -> String {
    if state.tab == "files" {
        return render_files_html(state);
    }
    render_metadata_html(state)
}

fn render_files_html(state: &UiStateDto) -> String {
    let rows = state
        .files
        .iter()
        .map(|file| {
            let bytes = file
                .bytes
                .map(|value| format!("{value} bytes"))
                .unwrap_or_default();
            format!(
                r#"<div class="file-row"><span>{}</span><span>{}</span><span>{}</span></div>"#,
                escape_text(&file.path),
                escape_text(&file.kind),
                escape_text(&bytes)
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(r#"<div class="files-list">{rows}</div>"#)
}

fn render_metadata_html(state: &UiStateDto) -> String {
    let category_summary = render_category_summary_html(state);
    let Some(detail) = &state.active_detail else {
        return format!(
            r#"<div class="metadata-layout">{category_summary}{}</div>"#,
            render_empty_metadata_form(state)
        );
    };
    format!(
        r#"<div class="metadata-layout">{category_summary}<form method="post" action="/ui/action" class="metadata-form"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="save_metadata"/><input type="hidden" name="item_id" value="{item_id}"/><input type="hidden" name="expected_revision" value="{revision}"/><label class="field"><span>Title</span><input name="title" value="{title}"/></label><label class="field"><span>Authors</span><input name="authors" value="{authors}"/></label><label class="field"><span>Type</span><input name="item_type" value="{item_type}"/></label><button class="button primary wide" type="submit">Save Metadata</button></form></div>"#,
        category_summary = category_summary,
        return_to = escape_attr(&state.return_to),
        item_id = escape_attr(state.active_id.as_deref().unwrap_or_default()),
        revision = escape_attr(&detail.metadata_revision),
        title = escape_attr(&detail.title),
        authors = escape_attr(&detail.authors),
        item_type = escape_attr(&detail.item_type),
    )
}

fn render_empty_metadata_form(state: &UiStateDto) -> String {
    format!(
        r#"<form method="post" action="/ui/action" class="metadata-form"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="save_metadata"/><input type="hidden" name="item_id" value=""/><input type="hidden" name="expected_revision" value=""/><label class="field"><span>Title</span><input name="title" value=""/></label><label class="field"><span>Authors</span><input name="authors" value=""/></label><label class="field"><span>Type</span><input name="item_type" value=""/></label><label class="field"><span>Year</span><input name="year" value=""/></label><label class="field"><span>DOI</span><input name="doi" value=""/></label><label class="field"><span>Venue</span><input name="venue" value=""/></label><label class="field"><span>Language</span><input name="language" value=""/></label><label class="field"><span>URI</span><input name="uri" value=""/></label><label class="field wide">Abstract<textarea name="abstract_note"></textarea></label><button class="button primary wide" type="submit">Save Metadata</button></form>"#,
        return_to = escape_attr(&state.return_to),
    )
}

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

fn escape_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attr(value: &str) -> String {
    escape_text(value).replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::{render_detail_html, render_item_list_html};
    use crate::api::UiStateDto;

    #[test]
    fn item_list_marks_active_and_checked_rows() {
        let state = state_fixture("metadata");

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

        assert!(html.contains("paper.pdf"));
        assert!(html.contains("12 bytes"));
        assert!(!html.contains("Save Metadata"));
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
                "events": [],
                "pending_count": 0,
                "selected_ids": ["lr:zotero:alpha"],
                "category_target_ids": ["lr:zotero:alpha"],
                "active_id": "lr:zotero:alpha",
                "active_detail": {{"metadata_revision": "rev1", "title": "Alpha & Beta", "authors": "Ada", "item_type": "journalArticle", "year": 2026, "doi": null, "venue": null, "language": "en", "uri": null, "abstract_note": "Abstract"}},
                "tab": "{tab}",
                "return_to": "/?active=lr:zotero:alpha&tab={tab}",
                "status_label": "Running",
                "files": [{{"path": "paper.pdf", "kind": "file", "bytes": 12}}],
                "rules_notice": null
            }}"#,
        ))
        .unwrap()
    }
}
