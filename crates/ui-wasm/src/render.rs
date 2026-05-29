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
    let Some(detail) = &state.active_detail else {
        return r#"<div class="metadata-layout"></div>"#.to_string();
    };
    format!(
        r#"<div class="metadata-layout"><form method="post" action="/ui/action" class="metadata-form"><input type="hidden" name="return_to" value="{return_to}"/><input type="hidden" name="action" value="save_metadata"/><input type="hidden" name="item_id" value="{item_id}"/><input type="hidden" name="expected_revision" value="{revision}"/><label class="field"><span>Title</span><input name="title" value="{title}"/></label><label class="field"><span>Authors</span><input name="authors" value="{authors}"/></label><label class="field"><span>Type</span><input name="item_type" value="{item_type}"/></label><button class="button primary wide" type="submit">Save Metadata</button></form></div>"#,
        return_to = escape_attr(&state.return_to),
        item_id = escape_attr(state.active_id.as_deref().unwrap_or_default()),
        revision = escape_attr(&detail.metadata_revision),
        title = escape_attr(&detail.title),
        authors = escape_attr(&detail.authors),
        item_type = escape_attr(&detail.item_type),
    )
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
