//! Localref `mdnote` plugin: keep a Markdown note beside each item.
//!
//! Both actions bind to the active item. They ensure `note.md` exists in the
//! item's folder (seeded with `# {item title}` on first creation), then open it
//! in the system default Markdown editor. `open_note_with_file` also opens the
//! item's main file so note and source sit side by side.
//!
//! ```text
//! mdnote run open_note           --endpoint http://127.0.0.1:24817 --active lr:zotero:a
//! mdnote run open_note_with_file --endpoint http://127.0.0.1:24817 --active lr:zotero:a
//! ```

use std::path::Path;

use localref_core::config::LocalrefConfig;
use localref_plugin_sdk::{
    ActionContext, Invocation, LocalrefClient, LogLevel, RunOutput, emit,
    parse_args,
};

/// Plugin name used as the per-plugin log target on the host.
const PLUGIN_NAME: &str = "mdnote";
/// Fixed note filename created in each item's folder.
const NOTE_NAME: &str = "note.md";

#[tokio::main]
async fn main() {
    let Some(invocation) = parse_args(std::env::args().skip(1)) else {
        emit(&RunOutput::error(
            "usage: mdnote run <action> --endpoint … --active …",
        ));
        return;
    };
    match invocation {
        Invocation::Manifest => {
            println!("mdnote — create and open a Markdown note per item");
        }
        Invocation::Run { action, endpoint, selected, active, params } => {
            let ctx = ActionContext {
                selected,
                active,
                params,
                client: LocalrefClient::new(&endpoint),
            };
            emit(&run(&action, &ctx).await);
        }
        // mdnote is interactive-only; it declares no hooks or cron jobs.
        Invocation::Hook { .. } | Invocation::Cron { .. } => {
            emit(&RunOutput::error("mdnote has no hook or cron entry points"));
        }
    }
}

/// Dispatch a `run` action. Both actions open the note; the second also opens
/// the item's main file. Opening is a side effect with no savable output, so
/// success is always [`RunOutput::done`].
async fn run(action: &str, ctx: &ActionContext) -> RunOutput {
    let with_main_file = match action {
        "open_note" => false,
        "open_note_with_file" => true,
        other => return RunOutput::error(format!("unknown action: {other}")),
    };
    match open_note(ctx, with_main_file).await {
        Ok(()) => RunOutput::done(),
        Err(e) => RunOutput::error(e),
    }
}

/// Ensure `note.md` exists in the active item's folder, then open it (and,
/// when `with_main_file` is set, the item's main file too).
async fn open_note(
    ctx: &ActionContext,
    with_main_file: bool,
) -> Result<(), String> {
    let item_id = ctx.active.as_deref().ok_or("no active item")?;
    let item =
        ctx.client.get_item(item_id).await.map_err(|e| e.to_string())?;

    let library_root = LocalrefConfig::load()?.library_root().to_path_buf();
    let item_dir = library_root.join(&item.object_path);
    let note_path = item_dir.join(NOTE_NAME);

    let created = ensure_note(&note_path, &item.title)?;
    let verb = if created { "created and opened" } else { "opened" };
    log(ctx, LogLevel::Info, &format!("{verb} {}", note_path.display())).await;

    open_path(&note_path)?;
    if with_main_file {
        open_main_file(ctx, &item, &item_dir).await;
    }
    Ok(())
}

/// Create the note seeded with `# {title}` if it does not yet exist. Returns
/// `true` when a new file was written, `false` when one was already present
/// (so an existing note is never overwritten).
fn ensure_note(note_path: &Path, title: &str) -> Result<bool, String> {
    if note_path.exists() {
        return Ok(false);
    }
    if let Some(parent) = note_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(note_path, format!("# {title}\n"))
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Open a path in the system default application.
fn open_path(path: &Path) -> Result<(), String> {
    open::that(path)
        .map_err(|e| format!("could not open {}: {e}", path.display()))
}

/// Open the item's main file, if it has one. Best-effort: a missing main file
/// or a failed open is logged, not fatal — the note itself is already open.
async fn open_main_file(
    ctx: &ActionContext,
    item: &localref_plugin_sdk::ItemDoc,
    item_dir: &Path,
) {
    let Some(main) = item.main_file.as_deref() else {
        log(ctx, LogLevel::Warn, "item has no main file to open").await;
        return;
    };
    if let Err(e) = open_path(&item_dir.join(main)) {
        log(ctx, LogLevel::Warn, &e).await;
    }
}

/// Forward a line into the daemon's unified log under
/// `localref::plugin::mdnote`. Best-effort: a logging failure never fails the
/// action.
async fn log(ctx: &ActionContext, level: LogLevel, message: &str) {
    let _ = ctx.client.log(PLUGIN_NAME, level, message).await;
}
