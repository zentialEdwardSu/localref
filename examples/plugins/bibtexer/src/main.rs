//! Example Localref plugin implemented as a plain stdin/stdout CLI.
//!
//! The host sends one JSON object to stdin and expects one JSON object on
//! stdout. The plugin may return structured action results or rendered HTML
//! fragments for fixed UI mount points.

use std::collections::HashMap;
use std::io::Read;

use localref_plugin::{
    PluginItemSummary, PluginUiState, RenderOutput, RunOutput,
};
use serde::Deserialize;

/// CLI request envelope sent by the Localref host.
#[derive(Debug, Deserialize)]
struct PluginInput {
    /// Invocation mode: manifest, render, or run.
    mode: String,
    /// Page id for render mode.
    #[serde(default)]
    page: String,
    /// Action id for run mode.
    #[serde(default)]
    action: String,
    /// Form/action parameters supplied by the mounted web page or host UI.
    #[serde(default)]
    params: HashMap<String, String>,
    /// Current Localref UI data supplied by the host.
    #[serde(default)]
    state: Option<PluginUiState>,
}

/// Read one request from stdin, process it, and print one JSON response.
fn main() {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        print_json(&RunOutput::error("failed to read stdin"));
        return;
    }
    let Ok(input) = serde_json::from_str::<PluginInput>(input.trim()) else {
        print_json(&RunOutput::error("invalid plugin input JSON"));
        return;
    };
    match input.mode.as_str() {
        "manifest" => print_manifest(),
        "render" => print_json(&render(&input)),
        "run" => print_json(&run(&input)),
        mode => print_json(&RunOutput::error(format!("unknown mode: {mode}"))),
    }
}

/// Print the plugin manifest for hosts that discover capabilities by CLI.
fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "bibtexer",
        "executable": "bibtexer",
        "description": "Export citations in BibTeX and RIS formats",
        "actions": [
            {"id": "export_bibtex", "label": "BibTeX", "mount": "action_button"},
            {"id": "export_ris", "label": "RIS", "mount": "context_menu"}
        ],
        "pages": [
            {"id": "export_form", "label": "Export", "mount": "detail_tab", "route": "export"},
            {"id": "metadata_export", "label": "Citation Export", "mount": "metadata_page", "route": "metadata-export"},
            {"id": "selection_export", "label": "Bulk Citation Export", "mount": "selection_page", "route": "selection-export"}
        ]
    });
    println!("{}", manifest);
}

/// Render one plugin HTML page.
fn render(input: &PluginInput) -> RenderOutput {
    let Some(state) = input.state.as_ref() else {
        return RenderOutput::error("render request missing state");
    };
    match input.page.as_str() {
        "export_form" | "metadata_export" | "selection_export" => {
            render_export_form(state)
        }
        page => RenderOutput::error(format!("unknown page: {page}")),
    }
}

/// Execute one plugin action and return structured output.
fn run(input: &PluginInput) -> RunOutput {
    let Some(state) = input.state.as_ref() else {
        return RunOutput::error("action request missing state");
    };
    match input.action.as_str() {
        "export_bibtex" => {
            let format =
                input.params.get("format").map_or("bibtex", String::as_str);
            export_citations(state, &input.params, format)
        }
        "export_ris" => export_citations(state, &input.params, "ris"),
        action => RunOutput::error(format!("unknown action: {action}")),
    }
}

/// Render a small form that posts actions back through the host action route.
fn render_export_form(state: &PluginUiState) -> RenderOutput {
    let ids = export_ids(state, &HashMap::new());
    let count = ids.len();
    let items = state
        .items
        .iter()
        .filter(|item| ids.contains(&item.id))
        .take(10)
        .map(render_item_preview)
        .collect::<Vec<_>>()
        .join("\n");
    let return_to = escape_html(&return_to(state));
    let html = format!(
        r#"<div class="plugin-bibtexer">
<form method="post" action="/plugin/bibtexer/action" class="export-form">
    <input type="hidden" name="plugin_action" value="export_bibtex"/>
    <input type="hidden" name="return_to" value="{return_to}"/>
    <section class="export-section">
        <h3>Export {count} items</h3>
        <fieldset class="format-select">
            <legend>Format</legend>
            <label><input type="radio" name="format" value="bibtex" checked/> BibTeX</label>
            <label><input type="radio" name="format" value="ris"/> RIS</label>
        </fieldset>
        <button class="button primary" type="submit">Export</button>
    </section>
    <section class="export-preview">
        <h4>Items</h4>
        <ul>{items}</ul>
    </section>
</form>
</div>"#
    );
    RenderOutput::ok(html)
}

/// Convert selected Localref items into a citation download.
fn export_citations(
    state: &PluginUiState,
    params: &HashMap<String, String>,
    format: &str,
) -> RunOutput {
    let ids = export_ids(state, params);
    if ids.is_empty() {
        return RunOutput::error("no items selected");
    }
    let citations = ids
        .iter()
        .filter_map(|id| state.items.iter().find(|item| &item.id == id))
        .map(|item| format_citation(item, format))
        .collect::<Vec<_>>()
        .join("\n\n");
    match format {
        "bibtex" => RunOutput::ok(citations)
            .content_type("text/x-bibtex")
            .filename("localref-export.bib"),
        "ris" => RunOutput::ok(citations)
            .content_type("application/x-research-info-systems")
            .filename("localref-export.ris"),
        _ => RunOutput::ok(citations)
            .content_type("text/plain")
            .filename("localref-export.txt"),
    }
}

/// Return item ids targeted by this action or page render.
fn export_ids(
    state: &PluginUiState,
    params: &HashMap<String, String>,
) -> Vec<String> {
    if let Some(item_ids) =
        params.get("item_ids").filter(|value| !value.trim().is_empty())
    {
        return item_ids
            .split(',')
            .filter(|id| !id.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    if state.selected_ids.is_empty() {
        state.active_id.iter().cloned().collect()
    } else {
        state.selected_ids.clone()
    }
}

/// Format one item in a tiny preview list.
fn render_item_preview(item: &PluginItemSummary) -> String {
    let title = escape_html(&item.title);
    let id = escape_html(&item.id);
    let authors = escape_html(&item.authors.join(", "));
    format!(
        "<li><strong>{title}</strong><br><small>{authors}</small><br><code>{id}</code></li>"
    )
}

/// Format one citation record.
fn format_citation(item: &PluginItemSummary, format: &str) -> String {
    let first_author = item.authors.first().map_or("Unknown", String::as_str);
    let year = "2025";
    match format {
        "bibtex" => {
            let key = first_author.to_lowercase().replace(' ', "") + year;
            format!(
                "@article{{{key},\n  author = {{{first_author}}},\n  title = {{{title}}},\n  year = {{{year}}}\n}}",
                title = item.title
            )
        }
        "ris" => {
            format!(
                "TY  - JOUR\nAU  - {first_author}\nTI  - {title}\nPY  - {year}\nER  - ",
                title = item.title
            )
        }
        _ => item.title.clone(),
    }
}

/// Build the host return URL from current route state.
fn return_to(state: &PluginUiState) -> String {
    let mut parts = Vec::new();
    if let Some(q) = state.search.as_deref() {
        parts.push(format!("q={}", encode_query(q)));
    }
    if let Some(category) = state.category.as_deref() {
        parts.push(format!("category={}", encode_query(category)));
    }
    if !state.selected_ids.is_empty() {
        parts.push(format!("selected={}", state.selected_ids.join(",")));
    }
    if let Some(active) = state.active_id.as_deref() {
        parts.push(format!("active={}", encode_query(active)));
    }
    parts.push(format!("tab={}", encode_query(&state.tab)));
    format!("/?{}", parts.join("&"))
}

/// Percent-encode a value for Localref's query string.
fn encode_query(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':' | b',')
        {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Escape raw text before embedding it into HTML.
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Serialize a response value to stdout.
fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string(value)
            .unwrap_or_else(|_| "{\"status\":\"error\"}".to_string())
    );
}
