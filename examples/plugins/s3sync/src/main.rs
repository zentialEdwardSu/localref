//! Localref `s3sync` plugin: sync each item's files to S3-compatible storage
//! using the [`rollforward`] engine, with version history and conflict handling.
//!
//! ```text
//! s3sync run sync_all      --endpoint http://127.0.0.1:24817
//! s3sync run sync_selected --endpoint … --selected lr:zotero:a,lr:zotero:b
//! s3sync run list_history  --endpoint … --active lr:zotero:a
//! s3sync run rollback      --endpoint … --active lr:zotero:a --param sequence=3
//! s3sync cron nightly_sync --endpoint …
//! ```
//!
//! Files are synced as binary (content-defined chunking); a genuinely divergent
//! binary edit is resolved with `KeepBoth` — the losing side is written into the
//! item as a `(conflict)` copy and the item's row is flagged red in the desktop
//! app via the reserved `ui.bar_color` extra.

mod config;
mod listener;
mod s3_remote;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use config::S3SyncConfig;
use listener::RecordingListener;
use localref_core::config::LocalrefConfig;
use localref_plugin_sdk::{
    ActionContext, Invocation, LocalrefClient, LogLevel, NotifyKind, RunOutput, emit, parse_args,
};
use object_store::ObjectStore;
use object_store::aws::{AmazonS3Builder, S3ConditionalPut};
use object_store::local::LocalFileSystem;
use rollforward::types::{BinaryConflictPolicy, OpLogEntry};
use rollforward::{RedbStore, RemoteStorage, SyncEngine};
use s3_remote::S3Remote;
use tokio::runtime::Handle;

/// Plugin name (log target and notification title).
const PLUGIN_NAME: &str = "s3sync";
/// Extra namespace for this plugin's per-item state.
const NS: &str = "s3sync";
/// Row color used to flag a conflicted item in the desktop list.
const CONFLICT_COLOR: &str = "#e11d48";

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let Some(invocation) = parse_args(std::env::args().skip(1)) else {
        emit(&RunOutput::error("usage: s3sync <run|cron> … --endpoint …"));
        return;
    };
    match invocation {
        Invocation::Manifest => {
            println!("s3sync — sync item files to S3 with history and conflict resolution");
        }
        Invocation::Run { action, endpoint, selected, active, params } => {
            let ctx = ActionContext { selected, active, params, client: LocalrefClient::new(&endpoint) };
            emit(&run(&action, &ctx).await);
        }
        Invocation::Cron { job, endpoint } => {
            let output = if job == "nightly_sync" {
                let client = LocalrefClient::new(&endpoint);
                let ctx = ActionContext { selected: vec![], active: None, params: Default::default(), client };
                let out = sync_all(&ctx).await;
                // Cron runs unattended, so surface the outcome as a notification.
                let (title, body, kind) = match &out {
                    RunOutput { status, result: Some(msg), .. } if status == "ok" => {
                        ("s3sync nightly sync", msg.clone(), NotifyKind::Success)
                    }
                    RunOutput { message: Some(msg), .. } => {
                        ("s3sync nightly sync failed", msg.clone(), NotifyKind::Error)
                    }
                    _ => ("s3sync nightly sync", "completed".to_owned(), NotifyKind::Info),
                };
                let _ = ctx.client.notify(title, &body, kind).await;
                out
            } else {
                RunOutput::error(format!("unknown cron job: {job}"))
            };
            emit(&output);
        }
        Invocation::Hook { .. } => {
            emit(&RunOutput::error("s3sync has no hook entry points"));
        }
    }
}

/// Forward a line into the daemon's unified log under `localref::plugin::s3sync`,
/// so plugin activity shows up alongside daemon logs in the app. Best-effort:
/// a logging transport failure must never fail the action itself.
async fn log(ctx: &ActionContext, level: LogLevel, message: &str) {
    let _ = ctx.client.log(PLUGIN_NAME, level, message).await;
}

/// Dispatch a `run` action.
async fn run(action: &str, ctx: &ActionContext) -> RunOutput {
    match action {
        "sync_selected" => sync_items(ctx, &target_ids(ctx)).await,
        "sync_all" => sync_all(ctx).await,
        "list_history" => list_history(ctx).await,
        "rollback" => rollback(ctx).await,
        "list_conflicts" => list_conflicts(ctx).await,
        "resolve_conflicts" => resolve_conflicts(ctx).await,
        other => RunOutput::error(format!("unknown action: {other}")),
    }
}

/// Resolve target ids: the checked selection, else the active item.
fn target_ids(ctx: &ActionContext) -> Vec<String> {
    if !ctx.selected.is_empty() {
        ctx.selected.clone()
    } else {
        ctx.active.iter().cloned().collect()
    }
}

/// Everything needed to run the engine for one plugin invocation.
struct Session {
    /// The configured sync engine.
    engine: Arc<SyncEngine>,
    /// The remote, kept for direct oplog/chunk reads (history, reassembly).
    remote: Arc<S3Remote>,
    /// Shared listener recording conflict/update notifications.
    listener: Arc<RecordingListener>,
    /// Absolute library root, for resolving item file paths.
    library_root: PathBuf,
}

/// Open a session, and if it fails — most commonly a missing or invalid
/// config — surface the error as a daemon notification before returning it.
/// Action output isn't always visible to the user (e.g. a triggered run), so
/// the notification is what makes a setup problem noticeable.
async fn open_session_notified(
    ctx: &ActionContext,
    policy: BinaryConflictPolicy,
) -> Result<Session, String> {
    match open_session(policy) {
        Ok(session) => Ok(session),
        Err(e) => {
            let _ = ctx.client.notify(PLUGIN_NAME, &e, NotifyKind::Error).await;
            Err(e)
        }
    }
}

/// Build an engine session from the plugin + library config.
///
/// `policy` selects the binary conflict resolution for this invocation.
fn open_session(policy: BinaryConflictPolicy) -> Result<Session, String> {
    let lr = LocalrefConfig::load()?;
    let library_root = lr.library_root().to_path_buf();
    let cfg = S3SyncConfig::load(&library_root)?;

    let store: Arc<dyn ObjectStore> = build_object_store(&cfg)?;
    let handle = Handle::current();
    let remote = Arc::new(S3Remote::new(store, cfg.prefix.clone(), handle));

    let store_path = config::plugin_dir(&library_root).join("store.redb");
    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let local = Arc::new(RedbStore::open(&store_path).map_err(|e| e.to_string())?);

    let listener = Arc::new(RecordingListener::default());
    let engine = Arc::new(SyncEngine::with_backends(
        cfg.client_id.clone(),
        local,
        remote.clone(),
        listener.clone(),
        policy,
    ));
    Ok(Session { engine, remote, listener, library_root })
}

/// Construct the object store from config: a custom/AWS S3 endpoint, or (when
/// `bucket` is a `file://` or absolute path) a local filesystem for testing.
fn build_object_store(cfg: &S3SyncConfig) -> Result<Arc<dyn ObjectStore>, String> {
    if let Some(local) = cfg.bucket.strip_prefix("file://") {
        return Ok(Arc::new(LocalFileSystem::new_with_prefix(local).map_err(|e| e.to_string())?));
    }
    // Start from the AWS environment chain, then let explicit config values
    // (credentials, region, endpoint) override it. This keeps env-based setups
    // working while making an R2/MinIO config a single-file edit.
    //
    // `ETagMatch` conditional puts are required for `PutMode::Create` (used by
    // the engine's oplog CAS): object_store's S3 backend returns
    // `NotImplemented` for create/update-if-absent unless a conditional-put
    // strategy is set. R2, MinIO and modern S3 all honor `If-None-Match`.
    let mut builder = AmazonS3Builder::from_env()
        .with_bucket_name(&cfg.bucket)
        .with_allow_http(cfg.allow_http)
        .with_conditional_put(S3ConditionalPut::ETagMatch);
    if let Some(region) = &cfg.region {
        builder = builder.with_region(region);
    }
    if let Some(endpoint) = &cfg.endpoint {
        builder = builder.with_endpoint(endpoint);
    }
    if let Some(access_key_id) = &cfg.access_key_id {
        builder = builder.with_access_key_id(access_key_id);
    }
    if let Some(secret_access_key) = &cfg.secret_access_key {
        builder = builder.with_secret_access_key(secret_access_key);
    }
    if let Some(session_token) = &cfg.session_token {
        builder = builder.with_token(session_token);
    }
    if let Some(proxy) = &cfg.proxy {
        builder = builder.with_proxy_url(proxy.to_url()?);
    }
    Ok(Arc::new(builder.build().map_err(|e| e.to_string())?))
}

/// The engine `file_id` for an item's relative file path.
fn file_id_for(item_id: &str, rel: &str) -> String {
    format!("{item_id}/{rel}")
}

/// Absolute on-disk path of an item file: `library_root/object_path/rel`.
fn abs_path(library_root: &Path, object_path: &str, rel: &str) -> PathBuf {
    library_root.join(object_path).join(rel)
}

/// Sync every item in the library.
async fn sync_all(ctx: &ActionContext) -> RunOutput {
    let items = match ctx.client.list_items().await {
        Ok(items) => items,
        Err(e) => return RunOutput::error(format!("failed to list items: {e}")),
    };
    let ids: Vec<String> = items.into_iter().map(|item| item.id).collect();
    sync_items(ctx, &ids).await
}

/// Sync the given items: push each local file, pull+merge, reassemble, and flag
/// any conflicts. Returns a human summary.
async fn sync_items(ctx: &ActionContext, item_ids: &[String]) -> RunOutput {
    if item_ids.is_empty() {
        return RunOutput::error("no items to sync");
    }
    let session = match open_session_notified(ctx, BinaryConflictPolicy::KeepBoth).await {
        Ok(s) => s,
        Err(e) => return RunOutput::error(e),
    };
    log(ctx, LogLevel::Info, &format!("starting sync of {} item(s)", item_ids.len())).await;

    let mut synced_files = 0usize;
    let mut conflicts = 0usize;
    for item_id in item_ids {
        match sync_one_item(ctx, &session, item_id).await {
            Ok((files, item_conflicts)) => {
                synced_files += files;
                conflicts += item_conflicts;
            }
            Err(e) => {
                let msg = format!("sync failed for {item_id}: {e}");
                log(ctx, LogLevel::Warn, &msg).await;
                return RunOutput::error(msg);
            }
        }
    }
    let summary = format!(
        "Synced {synced_files} file(s) across {} item(s); {conflicts} conflict(s)",
        item_ids.len()
    );
    log(ctx, LogLevel::Info, &summary).await;
    RunOutput::ok(summary)
}

/// Sync a single item's files. Returns `(files_synced, conflicts_flagged)`.
async fn sync_one_item(
    ctx: &ActionContext,
    session: &Session,
    item_id: &str,
) -> Result<(usize, usize), String> {
    let item = ctx.client.get_item(item_id).await.map_err(|e| e.to_string())?;
    let files = ctx.client.item_files(item_id).await.map_err(|e| e.to_string())?;

    let mut count = 0usize;
    for entry in &files.files {
        if entry.kind != "file" {
            continue;
        }
        let file_id = file_id_for(item_id, &entry.path);
        let path = abs_path(&session.library_root, &item.object_path, &entry.path);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            // A listed entry that can't be read (race, permission) is skipped,
            // not fatal to the whole item.
            Err(_) => continue,
        };
        // Push local content, then pull+merge the converged state.
        session.engine.modify_binary(file_id.clone(), bytes).map_err(|e| e.to_string())?;
        session.engine.sync(file_id.clone()).map_err(|e| e.to_string())?;
        // Reassemble the converged manifest back to disk if it changed.
        reassemble_to_disk(session, &file_id, &path)?;
        count += 1;
    }

    // Handle any conflict copies the engine requested during the syncs above.
    let conflicts = session.listener.take_conflicts();
    let n_conflicts = conflicts.len();
    if n_conflicts == 0 {
        // Clear any stale conflict flag and mark synced.
        let _ = ctx.client.set_item_extra(item_id, NS, "status", Some("synced")).await;
        let _ = ctx.client.set_bar_color(item_id, None).await;
    } else {
        for conflict in conflicts {
            write_conflict_copy(ctx, session, item_id, &conflict).await?;
        }
        let _ = ctx.client.set_item_extra(item_id, NS, "status", Some("conflict")).await;
        let _ = ctx.client.set_bar_color(item_id, Some(CONFLICT_COLOR)).await;
        log(ctx, LogLevel::Warn, &format!("{item_id}: {n_conflicts} conflict(s) kept as copies")).await;
    }
    let _ = session.listener.take_updated();
    Ok((count, n_conflicts))
}

/// Reassemble a file's converged chunk manifest from the remote and write it to
/// `dest`, but only when the bytes differ from what is already on disk.
fn reassemble_to_disk(session: &Session, file_id: &str, dest: &Path) -> Result<(), String> {
    let manifest = session.engine.get_manifest(file_id.to_owned()).map_err(|e| e.to_string())?;
    let mut content = Vec::new();
    for hash in &manifest {
        let chunk = session.remote.get_chunk(hash.clone()).map_err(|e| e.to_string())?;
        content.extend_from_slice(&chunk);
    }
    // Skip the write when unchanged to avoid churning mtimes / rescans.
    if std::fs::read(dest).is_ok_and(|existing| existing == content) {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(dest, &content).map_err(|e| e.to_string())
}

/// Write a "keep both" conflict copy into the item directory and register it.
async fn write_conflict_copy(
    ctx: &ActionContext,
    session: &Session,
    item_id: &str,
    conflict: &listener::ConflictCopy,
) -> Result<(), String> {
    // The engine's current merged state is the kept copy for this branch. Write
    // it to a temp file named with the suggested conflict name, then hand that
    // path to the daemon: `add_file` copies it into the item directory under a
    // managed, sanitized name and records it in the item's metadata (so we
    // don't drop an unmanaged file the daemon knows nothing about).
    let leaf = conflict
        .suggested_name
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("conflict-copy");
    let tmp_dir = std::env::temp_dir().join("localref-s3sync");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let tmp_path = tmp_dir.join(leaf);
    reassemble_to_disk(session, &conflict.file_id, &tmp_path)?;

    let path_str = tmp_path.to_string_lossy().into_owned();
    let result = ctx.client.add_file(item_id, &path_str).await.map_err(|e| e.to_string());
    // Best-effort cleanup of the staging copy regardless of the add outcome.
    let _ = std::fs::remove_file(&tmp_path);
    result.map(|_| ())?;

    let _ = ctx.client.set_item_extra(item_id, NS, "last_conflict", Some(leaf)).await;
    Ok(())
}

/// Resolve the `file_id` for the active item's history/rollback view: its main
/// file, else its first listed file.
async fn active_file_id(ctx: &ActionContext) -> Result<String, String> {
    let item_id = ctx.active.clone().ok_or("no active item")?;
    let item = ctx.client.get_item(&item_id).await.map_err(|e| e.to_string())?;
    let rel = if let Some(main) = item.main_file {
        main
    } else {
        let files = ctx.client.item_files(&item_id).await.map_err(|e| e.to_string())?;
        files
            .files
            .into_iter()
            .find(|f| f.kind == "file")
            .map(|f| f.path)
            .ok_or("item has no files to show history for")?
    };
    Ok(file_id_for(&item_id, &rel))
}

/// List the version history (oplog) of the active item's file as text.
async fn list_history(ctx: &ActionContext) -> RunOutput {
    let file_id = match active_file_id(ctx).await {
        Ok(id) => id,
        Err(e) => return RunOutput::error(e),
    };
    let session = match open_session_notified(ctx, BinaryConflictPolicy::KeepBoth).await {
        Ok(s) => s,
        Err(e) => return RunOutput::error(e),
    };
    let mut items = match session.remote.list_oplogs(file_id.clone()) {
        Ok(items) => items,
        Err(e) => return RunOutput::error(format!("failed to list history: {e}")),
    };
    items.sort_by(|a, b| a.sequence.cmp(&b.sequence).then(a.client_id.cmp(&b.client_id)));
    if items.is_empty() {
        return RunOutput::ok("No versions yet — sync this item first.".to_owned());
    }
    let mut lines = Vec::with_capacity(items.len());
    for item in &items {
        let stamp = match session.remote.get_oplog(file_id.clone(), item.remote_path.clone()) {
            Ok(bytes) => serde_json::from_slice::<OpLogEntry>(&bytes)
                .map(|e| format_stamp(e.timestamp))
                .unwrap_or_else(|_| "?".to_owned()),
            Err(_) => "?".to_owned(),
        };
        lines.push(format!("seq {} · {} · {}", item.sequence, stamp, item.client_id));
    }
    RunOutput::ok(lines.join("\n"))
}

/// Roll the active item's file back to a chosen sequence.
async fn rollback(ctx: &ActionContext) -> RunOutput {
    let seq: u64 = match ctx.params.get("sequence").and_then(|s| s.trim().parse().ok()) {
        Some(seq) => seq,
        None => return RunOutput::error("provide a numeric `sequence` to roll back to"),
    };
    let file_id = match active_file_id(ctx).await {
        Ok(id) => id,
        Err(e) => return RunOutput::error(e),
    };
    let session = match open_session_notified(ctx, BinaryConflictPolicy::KeepBoth).await {
        Ok(s) => s,
        Err(e) => return RunOutput::error(e),
    };
    // Make sure local engine state reflects the remote before rolling back.
    if let Err(e) = session.engine.sync(file_id.clone()) {
        return RunOutput::error(format!("sync before rollback failed: {e}"));
    }
    let new_seq = match session.engine.rollback(file_id.clone(), seq) {
        Ok(s) => s,
        Err(e) => return RunOutput::error(format!("rollback failed: {e}")),
    };
    // Reassemble the rolled-back content to disk.
    let (Some(item_id), rel) = split_file_id(&file_id) else {
        return RunOutput::error("rollback: could not resolve item from file id");
    };
    if let Ok(item) = ctx.client.get_item(item_id).await {
        let dest = abs_path(&session.library_root, &item.object_path, rel);
        if let Err(e) = reassemble_to_disk(&session, &file_id, &dest) {
            return RunOutput::error(format!("rollback wrote no file: {e}"));
        }
    }
    let msg = format!("Rolled back to seq {seq} (new version seq {new_seq})");
    let _ = ctx.client.notify(PLUGIN_NAME, &msg, NotifyKind::Success).await;
    RunOutput::ok(msg)
}

/// List items currently flagged as conflicted (via the indexed extra field).
async fn list_conflicts(ctx: &ActionContext) -> RunOutput {
    let items = match ctx.client.list_items().await {
        Ok(items) => items,
        Err(e) => return RunOutput::error(format!("failed to list items: {e}")),
    };
    let conflicted: Vec<String> = items
        .into_iter()
        .filter(|item| {
            item.extra
                .get(NS)
                .and_then(|ns| ns.get("status"))
                .map(|s| s == "conflict")
                .unwrap_or(false)
        })
        .map(|item| format!("{} — {}", item.title, item.id))
        .collect();
    if conflicted.is_empty() {
        RunOutput::ok("No conflicts.".to_owned())
    } else {
        RunOutput::ok(conflicted.join("\n"))
    }
}

/// Re-sync all conflicted items under the chosen policy.
async fn resolve_conflicts(ctx: &ActionContext) -> RunOutput {
    let policy = match ctx.params.get("policy").map(String::as_str) {
        Some("keep_local") => BinaryConflictPolicy::KeepLocal,
        Some("keep_remote") => BinaryConflictPolicy::KeepRemote,
        _ => BinaryConflictPolicy::KeepBoth,
    };
    let items = match ctx.client.list_items().await {
        Ok(items) => items,
        Err(e) => return RunOutput::error(format!("failed to list items: {e}")),
    };
    let conflicted: Vec<String> = items
        .into_iter()
        .filter(|item| {
            item.extra.get(NS).and_then(|ns| ns.get("status")).map(|s| s == "conflict").unwrap_or(false)
        })
        .map(|item| item.id)
        .collect();
    if conflicted.is_empty() {
        return RunOutput::ok("No conflicts to resolve.".to_owned());
    }
    let session = match open_session_notified(ctx, policy).await {
        Ok(s) => s,
        Err(e) => return RunOutput::error(e),
    };
    let mut resolved = 0usize;
    for item_id in &conflicted {
        if sync_one_item(ctx, &session, item_id).await.is_ok() {
            resolved += 1;
        }
    }
    let msg = format!("Resolved {resolved}/{} conflicted item(s)", conflicted.len());
    let _ = ctx.client.notify(PLUGIN_NAME, &msg, NotifyKind::Success).await;
    RunOutput::ok(msg)
}

/// Split `"{item_id}/{rel}"` back into `(item_id, rel)`. item ids are
/// `lr:<connector>:<id>` and contain no `/`, so the first `/` is the boundary.
fn split_file_id(file_id: &str) -> (Option<&str>, &str) {
    match file_id.split_once('/') {
        Some((id, rel)) => (Some(id), rel),
        None => (None, file_id),
    }
}

/// Format an epoch-millis timestamp as `YYYY-MM-DD HH:MM` UTC without pulling a
/// date library. Good enough for a human-readable history list.
fn format_stamp(millis: i64) -> String {
    if millis <= 0 {
        return "—".to_owned();
    }
    let secs = millis / 1000;
    let days = secs / 86_400;
    let tod = secs % 86_400;
    let (hh, mm) = (tod / 3600, (tod % 3600) / 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {hh:02}:{mm:02}")
}

/// Convert days-since-Unix-epoch to a `(year, month, day)` civil date
/// (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
