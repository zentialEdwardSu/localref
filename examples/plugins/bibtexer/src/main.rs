//! Example Localref plugin: a standalone CLI that talks to the daemon over
//! REST and prints one JSON result envelope. Run it directly:
//!
//! ```text
//! bibtexer run export_bibtex --endpoint http://127.0.0.1:8787 \
//!     --selected lr:zotero:a,lr:zotero:b --param format=bibtex
//! ```

use localref_plugin_sdk::{
    ActionContext, Invocation, LocalrefClient, RunOutput, emit, parse_args,
};

#[tokio::main]
async fn main() {
    let Some(invocation) = parse_args(std::env::args().skip(1)) else {
        emit(&RunOutput::error("usage: bibtexer run <action> --endpoint …"));
        return;
    };
    match invocation {
        Invocation::Manifest => {
            // Author self-check: identity only; the host reads plugin.toml.
            println!(
                "bibtexer (BibTeX/RIS export) — see plugin.toml / ui.toml"
            );
        }
        Invocation::Run { action, endpoint, selected, active, params } => {
            let ctx = ActionContext {
                selected,
                active,
                params,
                client: LocalrefClient::new(endpoint),
            };
            emit(&run(&action, &ctx).await);
        }
        // bibtexer is interactive-only; it declares no hooks or cron jobs.
        Invocation::Hook { .. } | Invocation::Cron { .. } => {
            emit(&RunOutput::error(
                "bibtexer has no hook or cron entry points",
            ));
        }
    }
}

/// Dispatch a run action.
async fn run(action: &str, ctx: &ActionContext) -> RunOutput {
    match action {
        "export_bibtex" => {
            let format =
                ctx.params.get("format").map_or("bibtex", String::as_str);
            export(ctx, format).await
        }
        "export_ris" => export(ctx, "ris").await,
        "preview_export" => match export(ctx, "bibtex").await {
            // Strip the filename so the result is shown inline, never saved.
            // The host only opens a save dialog when `filename` is set, so a
            // bare `ok(text)` is the canonical "display, don't download" form —
            // exactly what a preview wants (and what a run of this action gives).
            RunOutput { result: Some(text), .. } => RunOutput::ok(text),
            other => other,
        },
        other => RunOutput::error(format!("unknown action: {other}")),
    }
}

/// Fetch the targeted items via REST and format them.
async fn export(ctx: &ActionContext, format: &str) -> RunOutput {
    let ids = target_ids(ctx);
    if ids.is_empty() {
        return RunOutput::error("no items selected");
    }
    let mut records = Vec::new();
    for id in &ids {
        match ctx.client.get_item(id).await {
            Ok(item) => records.push(format_citation(&item, format)),
            Err(error) => {
                return RunOutput::error(format!(
                    "failed to read {id}: {error}"
                ));
            }
        }
    }
    let body = records.join("\n\n");
    match format {
        "ris" => RunOutput::ok(body)
            .content_type("application/x-research-info-systems")
            .filename("localref-export.ris"),
        _ => RunOutput::ok(body)
            .content_type("text/x-bibtex")
            .filename("localref-export.bib"),
    }
}

/// Resolve the target ids (selection wins, else the active item).
fn target_ids(ctx: &ActionContext) -> Vec<String> {
    if !ctx.selected.is_empty() {
        ctx.selected.clone()
    } else {
        ctx.active.iter().cloned().collect()
    }
}

/// Format one item as a BibTeX or RIS record.
fn format_citation(
    item: &localref_plugin_sdk::ItemDoc,
    format: &str,
) -> String {
    let first = item.authors.first().map_or("Unknown", String::as_str);
    let year = item.year.map_or_else(|| "n.d.".to_string(), |y| y.to_string());
    match format {
        "ris" => format!(
            "TY  - JOUR\nAU  - {first}\nTI  - {}\nPY  - {year}\nER  - ",
            item.title
        ),
        _ => {
            let key =
                format!("{}{year}", first.to_lowercase().replace(' ', ""));
            format!(
                "@article{{{key},\n  author = {{{first}}},\n  title = {{{}}},\n  year = {{{year}}}\n}}",
                item.title
            )
        }
    }
}
