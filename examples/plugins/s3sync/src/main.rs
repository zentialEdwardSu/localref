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

mod baseline;
mod config;
mod listener;
mod s3_remote;
mod sync_state;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use config::{Backend, S3SyncConfig};
use listener::RecordingListener;
use localref_core::config::LocalrefConfig;
use localref_plugin_sdk::{
    ActionContext, Invocation, LocalrefClient, LogLevel, NotifyKind, RunOutput, emit, parse_args,
};
use object_store::aws::{AmazonS3Builder, S3ConditionalPut};
use object_store::http::HttpBuilder;
use object_store::local::LocalFileSystem;
use object_store::{ClientOptions, ObjectStore};
use rollforward::binary;
use rollforward::types::{BinaryConflictPolicy, OpLogEntry};
use rollforward::{RedbStore, RemoteStorage, SyncEngine};
use s3_remote::S3Remote;
use sync_state::{ConflictRecord, SyncState};
use tokio::runtime::Handle;

/// Plugin name (log target and notification title).
const PLUGIN_NAME: &str = "s3sync";
/// Extra namespace for this plugin's per-item state.
const NS: &str = "s3sync";
/// Row color used to flag a conflicted item in the desktop list.
const CONFLICT_COLOR: &str = "#e11d48";
/// Structured preview payload consumed by schema-v2 plugin display panes.
const UI_JSON: &str = "application/vnd.localref.plugin-ui+json;v=1";

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
                // Cron runs unattended, so surface the outcome as a notification.
                let (out, title, body, kind) = match sync_all(&ctx).await {
                    Ok(msg) => (RunOutput::done(), "s3sync nightly sync", msg, NotifyKind::Success),
                    Err(msg) => {
                        (RunOutput::error(msg.clone()), "s3sync nightly sync failed", msg, NotifyKind::Error)
                    }
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

/// Push a live progress line to the desktop status bar. Best-effort like
/// [`log`]: a headless host simply drops it, and transport failures are ignored.
async fn set_status(ctx: &ActionContext, message: &str, kind: NotifyKind) {
    let _ = ctx.client.set_status(message, kind).await;
}

/// Report an informational, single-message result of an action: log it and
/// mirror it to the status bar. Used by actions whose whole output is one line
/// (counts, "nothing to do") so nothing is returned as a savable `result`.
async fn report(ctx: &ActionContext, message: &str, kind: NotifyKind) {
    log(ctx, LogLevel::Info, message).await;
    set_status(ctx, message, kind).await;
}

/// Dispatch a `run` action. Every action reports its outcome through the daemon
/// log and status bar (and notifications where unattended); none returns a
/// `result` payload, so the UI never opens a save dialog for what is purely
/// informational output. Success is therefore always [`RunOutput::done`].
async fn run(action: &str, ctx: &ActionContext) -> RunOutput {
    if action == "list_history" {
        return list_history(ctx).await.unwrap_or_else(RunOutput::error);
    }
    if action == "list_conflicts" {
        return list_conflicts_v2(ctx).await.unwrap_or_else(RunOutput::error);
    }
    let outcome = match action {
        "sync_selected" => sync_items(ctx, &target_ids(ctx)).await.map(drop),
        "sync_all" => sync_all(ctx).await.map(drop),
        "rollback" => rollback(ctx).await,
        "resolve_conflicts" => resolve_conflict_v2(ctx).await,
        "check_config" => check_config(ctx).await,
        other => Err(format!("unknown action: {other}")),
    };
    match outcome {
        Ok(()) => RunOutput::done(),
        Err(e) => RunOutput::error(e),
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
    /// Plugin state dir (`<library>/.localref/s3sync`), home of the baseline store.
    plugin_dir: PathBuf,
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
    let plugin_dir = config::plugin_dir(&library_root);

    let store: Arc<dyn ObjectStore> = build_object_store(&cfg)?;
    let handle = Handle::current();
    let remote = Arc::new(S3Remote::new(store, cfg.prefix.clone(), handle));

    let store_path = plugin_dir.join("store.redb");
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
    Ok(Session { engine, remote, listener, library_root, plugin_dir })
}

/// Construct the object store for the configured backend.
fn build_object_store(cfg: &S3SyncConfig) -> Result<Arc<dyn ObjectStore>, String> {
    match cfg.backend {
        Backend::S3 => build_s3_store(cfg),
        Backend::Http => build_http_store(cfg),
    }
}

/// Build the S3-compatible store. Credentials come only from config — the AWS
/// environment chain is not consulted, so the config file is the single source
/// of truth. When `bucket` is a `file://` path, a local filesystem is used for
/// testing instead.
fn build_s3_store(cfg: &S3SyncConfig) -> Result<Arc<dyn ObjectStore>, String> {
    if let Some(local) = cfg.bucket.strip_prefix("file://") {
        return Ok(Arc::new(LocalFileSystem::new_with_prefix(local).map_err(|e| e.to_string())?));
    }
    // `ETagMatch` conditional puts are required for `PutMode::Create` (used by
    // the engine's oplog CAS): object_store's S3 backend returns
    // `NotImplemented` for create/update-if-absent unless a conditional-put
    // strategy is set. R2, MinIO and modern S3 all honor `If-None-Match`.
    let mut builder = AmazonS3Builder::new()
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

/// Build the generic HTTP/WebDAV store. Basic auth (when configured) is sent as
/// an `Authorization` default header, since `HttpBuilder` has no native auth.
fn build_http_store(cfg: &S3SyncConfig) -> Result<Arc<dyn ObjectStore>, String> {
    let http = cfg
        .http
        .as_ref()
        .ok_or("backend `http` requires an `[http]` section with a `url`")?;

    let mut opts = ClientOptions::new().with_allow_http(cfg.allow_http);
    if let Some(proxy) = &cfg.proxy {
        opts = opts.with_proxy_url(proxy.to_url()?);
    }
    if let Some(header) = http.auth_header() {
        let value = http::HeaderValue::from_str(&header)
            .map_err(|e| format!("invalid auth header: {e}"))?;
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::AUTHORIZATION, value);
        opts = opts.with_default_headers(headers);
    }
    let store = HttpBuilder::new()
        .with_url(&http.url)
        .with_client_options(opts)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(store))
}

/// The engine `file_id` for an item's relative file path.
fn file_id_for(item_id: &str, rel: &str) -> String {
    format!("{item_id}/{rel}")
}

/// Absolute on-disk path of an item file: `library_root/object_path/rel`.
fn abs_path(library_root: &Path, object_path: &str, rel: &str) -> PathBuf {
    library_root.join(object_path).join(rel)
}

/// Sync every item in the library. Returns the human summary on success.
async fn sync_all(ctx: &ActionContext) -> Result<String, String> {
    let items = ctx
        .client
        .list_items()
        .await
        .map_err(|e| format!("failed to list items: {e}"))?;
    let ids: Vec<String> = items.into_iter().map(|item| item.id).collect();
    sync_items(ctx, &ids).await
}

/// Sync the given items: push each local file, pull+merge, reassemble, and flag
/// any conflicts. Reports progress via the status bar and log; returns the human
/// summary string on success (also logged and set as the final status).
async fn sync_items(ctx: &ActionContext, item_ids: &[String]) -> Result<String, String> {
    if item_ids.is_empty() {
        return Err("no items to sync".to_owned());
    }
    let session = open_session_notified(ctx, BinaryConflictPolicy::Manual).await?;
    log(ctx, LogLevel::Info, &format!("starting sync of {} item(s)", item_ids.len())).await;

    // The baseline store lets each file's sync choose push vs. pull correctly;
    // it is mutated per file and persisted once at the end, including on the
    // early-return path so a partial run still records the progress it made.
    let mut baselines = baseline::Baselines::load(&session.plugin_dir);
    let mut state = SyncState::load(&session.plugin_dir);
    let ordered_ids = state.pending_first(item_ids);

    let total = ordered_ids.len();
    let mut totals = SyncStats::default();
    for (index, item_id) in ordered_ids.iter().enumerate() {
        if state.blocks_item(item_id) {
            totals.conflicts += 1;
            log(ctx, LogLevel::Warn, &format!("{item_id}: sync paused pending manual resolution")).await;
            continue;
        }
        set_status(
            ctx,
            &format!("Syncing {item_id}, {}/{total}…", index + 1),
            NotifyKind::Info,
        )
        .await;
        match sync_one_item(ctx, &session, item_id, &mut baselines, &mut state).await {
            Ok(item_stats) => {
                totals.add(item_stats);
                if item_stats.conflicts == 0 {
                    state.complete_item(item_id);
                }
            }
            Err(e) => {
                let _ = baselines.save(&session.plugin_dir);
                let _ = state.save(&session.plugin_dir);
                let msg = format!("sync failed for {item_id}: {e}");
                log(ctx, LogLevel::Warn, &msg).await;
                set_status(ctx, &msg, NotifyKind::Error).await;
                return Err(msg);
            }
        }
    }
    if let Err(e) = baselines.save(&session.plugin_dir) {
        // Non-fatal: the sync itself converged; a lost baseline just makes the
        // next run fall back to the conservative push-and-KeepBoth path.
        log(ctx, LogLevel::Warn, &format!("could not persist sync baselines: {e}")).await;
    }
    if let Err(e) = state.save(&session.plugin_dir) {
        log(ctx, LogLevel::Warn, &format!("could not persist sync state: {e}")).await;
    }
    let summary = format!(
        "Synced {} item(s): {} pushed, {} pulled, {} skipped (in sync), {} conflict(s)",
        total, totals.pushed, totals.pulled, totals.skipped, totals.conflicts
    );
    log(ctx, LogLevel::Info, &summary).await;
    set_status(ctx, &summary, NotifyKind::Success).await;
    Ok(summary)
}

/// Sync a single item's files, logging each file's outcome (pushed / pulled /
/// skipped) so a run is auditable at the file level. Returns the item's
/// [`SyncStats`].
async fn sync_one_item(
    ctx: &ActionContext,
    session: &Session,
    item_id: &str,
    baselines: &mut baseline::Baselines,
    state: &mut SyncState,
) -> Result<SyncStats, String> {
    let item = ctx.client.get_item(item_id).await.map_err(|e| e.to_string())?;
    let files = ctx.client.item_files(item_id).await.map_err(|e| e.to_string())?;

    let mut stats = SyncStats::default();
    for entry in &files.files {
        if entry.kind != "file" {
            continue;
        }
        let file_id = file_id_for(item_id, &entry.path);
        let path = abs_path(&session.library_root, &item.object_path, &entry.path);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            // A listed entry that can't be read (race, permission) is skipped,
            // not fatal to the whole item — but say so, don't hide it.
            Err(e) => {
                log(ctx, LogLevel::Warn, &format!("{file_id}: unreadable, skipped ({e})")).await;
                continue;
            }
        };
        let local_manifest = binary::manifest(&bytes);
        let action = match sync_one_file(session, &file_id, &path, bytes, baselines) {
            Ok(action) => action,
            Err(error) if error.contains("user policy required") => {
                record_manual_conflict(ctx, state, item_id, &entry.path, &file_id, local_manifest, vec![]).await;
                stats.conflicts += 1;
                break;
            }
            Err(error) => return Err(error),
        };
        if action == FileAction::Conflict {
            let remote_manifest = session.engine.get_manifest(file_id.clone()).unwrap_or_default();
            record_manual_conflict(
                ctx,
                state,
                item_id,
                &entry.path,
                &file_id,
                local_manifest,
                remote_manifest,
            )
            .await;
            stats.conflicts += 1;
            break;
        }
        stats.record(action);
        // In-sync files are the common case and only noise at Info; log them at
        // Debug and reserve Info for files that actually moved.
        let level = if action == FileAction::InSync { LogLevel::Debug } else { LogLevel::Info };
        log(ctx, level, &format!("{file_id}: {}", action.log_verb())).await;
    }

    // A Manual policy never creates conflict copies. A conflict instead blocks
    // this item until the user chooses one of the explicit resolution actions.
    let conflicts = session.listener.take_conflicts();
    if stats.conflicts == 0 && conflicts.is_empty() {
        // Clear any stale conflict flag and mark synced.
        let _ = ctx.client.set_item_extra(item_id, NS, "status", Some("synced")).await;
        let _ = ctx.client.set_bar_color(item_id, None).await;
    } else {
        let _ = ctx.client.set_item_extra(item_id, NS, "status", Some("blocked")).await;
        let _ = ctx.client.set_bar_color(item_id, Some(CONFLICT_COLOR)).await;
        log(ctx, LogLevel::Warn, &format!("{item_id}: sync paused for manual conflict resolution")).await;
    }
    let _ = session.listener.take_updated();
    // Per-item roll-up so each item's contribution is visible without summing
    // the per-file lines by hand.
    log(
        ctx,
        LogLevel::Info,
        &format!(
            "{item_id}: {} pushed, {} pulled, {} skipped",
            stats.pushed, stats.pulled, stats.skipped
        ),
    )
    .await;
    Ok(stats)
}

async fn record_manual_conflict(
    ctx: &ActionContext,
    state: &mut SyncState,
    item_id: &str,
    relative_path: &str,
    file_id: &str,
    local_manifest: Vec<String>,
    remote_manifest: Vec<String>,
) {
    let detected_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0);
    state.record(ConflictRecord {
        id: file_id.to_owned(),
        item_id: item_id.to_owned(),
        file_id: file_id.to_owned(),
        relative_path: relative_path.to_owned(),
        detected_at_ms,
        local_manifest,
        remote_manifest,
    });
    let _ = ctx.client.set_item_extra(item_id, NS, "status", Some("blocked")).await;
    let _ = ctx.client.set_bar_color(item_id, Some(CONFLICT_COLOR)).await;
    let message = format!("Sync paused: {relative_path} has a conflict requiring manual resolution.");
    let _ = ctx.client.notify(PLUGIN_NAME, &message, NotifyKind::Error).await;
    set_status(ctx, &message, NotifyKind::Error).await;
}

/// The action a single file needs, decided from three manifests: what's on
/// disk now, what the remote converged to, and the baseline (what the disk
/// matched at the last sync).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileAction {
    /// Disk already equals the remote — nothing to do.
    InSync,
    /// Only the local side changed (or the file is new): publish the disk copy.
    Push,
    /// Only the remote side advanced; the disk is stale: write the remote copy
    /// down. This is the case the old code got wrong — it pushed the stale disk
    /// and forked the log.
    Pull,
    /// Both sides moved since the baseline and require a user decision.
    Conflict,
}

impl FileAction {
    /// Short past-tense verb for the sync log. `Conflict` is reported as a push
    /// because that is what physically happens — local is published and the
    /// engine's KeepBoth may then request a copy (counted separately below).
    fn log_verb(self) -> &'static str {
        match self {
            FileAction::InSync => "skipped (in sync)",
            FileAction::Pull => "pulled (remote advanced)",
            FileAction::Push => "pushed (local change)",
            FileAction::Conflict => "paused (manual conflict resolution required)",
        }
    }
}

/// Per-item tally of what the file-level sync did, so a run reports what it
/// actually moved rather than a single opaque count. `conflicts` counts
/// KeepBoth copies the engine requested, which is orthogonal to push/pull.
#[derive(Default, Clone, Copy)]
struct SyncStats {
    /// Files whose local edit (or first upload) was published.
    pushed: usize,
    /// Files whose stale disk copy was overwritten from the remote.
    pulled: usize,
    /// Files already byte-identical to the remote: nothing transferred.
    skipped: usize,
    /// KeepBoth conflict copies written for this scope.
    conflicts: usize,
}

impl SyncStats {
    /// Fold one file's decided action into the tally.
    fn record(&mut self, action: FileAction) {
        match action {
            FileAction::InSync => self.skipped += 1,
            FileAction::Pull => self.pulled += 1,
            FileAction::Push => self.pushed += 1,
            FileAction::Conflict => self.conflicts += 1,
        }
    }

    /// Accumulate another scope's tally into this one.
    fn add(&mut self, other: SyncStats) {
        self.pushed += other.pushed;
        self.pulled += other.pulled;
        self.skipped += other.skipped;
        self.conflicts += other.conflicts;
    }
}

/// Decide what a file needs from the local, remote, and baseline manifests.
/// `remote` is `None` when the file isn't tracked remotely yet (never synced).
fn decide_action(
    local: &[String],
    remote: Option<&[String]>,
    baseline: Option<&Vec<String>>,
) -> FileAction {
    let Some(remote) = remote else {
        // Not on the remote yet: a genuine first upload.
        return FileAction::Push;
    };
    if local == remote {
        return FileAction::InSync;
    }
    match baseline {
        // With a baseline we can attribute the divergence: if the disk still
        // matches the baseline, only the remote moved (pull); if the remote
        // still matches the baseline, only the disk moved (push); otherwise
        // both moved (conflict).
        Some(base) if base.as_slice() == local => FileAction::Pull,
        Some(base) if base.as_slice() == remote => FileAction::Push,
        Some(_) => FileAction::Conflict,
        // No baseline (first run after upgrade, or store lost): fall back to the
        // conservative push. The engine's KeepBoth guarantees no local edit is
        // dropped even if this push forks against a newer remote.
        None => FileAction::Push,
    }
}

/// Sync one file: pull the remote, decide push/pull/skip against the baseline,
/// act, and record the new baseline. Returns the [`FileAction`] taken so the
/// caller can log per-file what synced vs. what was skipped. Chunk-level dedup
/// (skipping chunks already stored remotely) happens inside the engine's
/// `upload_packs`; this level reports only whole-file outcomes.
fn sync_one_file(
    session: &Session,
    file_id: &str,
    path: &Path,
    bytes: Vec<u8>,
    baselines: &mut baseline::Baselines,
) -> Result<FileAction, String> {
    // Pull the remote's converged state first so the decision sees it. A
    // never-synced file has an empty oplog: `sync` is a no-op and `get_manifest`
    // then errors, which we read as "not tracked remotely" below.
    session.engine.sync(file_id.to_owned()).map_err(|e| e.to_string())?;
    let local = binary::manifest(&bytes);
    let remote = session.engine.get_manifest(file_id.to_owned()).ok();

    let action = decide_action(&local, remote.as_deref(), baselines.get(file_id));
    match action {
        FileAction::InSync => {
            // Record the converged manifest as the baseline so a later
            // remote-only change is correctly detected as a pull next run.
            baselines.set(file_id.to_owned(), local);
        }
        FileAction::Pull => {
            // Remote advanced, disk is stale: write the remote copy down.
            reassemble_to_disk(session, file_id, path)?;
            if let Ok(converged) = session.engine.get_manifest(file_id.to_owned()) {
                baselines.set(file_id.to_owned(), converged);
            }
        }
        FileAction::Push => {
            // Publish the local copy, re-sync to converge (a Conflict merges and
            // may request a KeepBoth copy), then write the converged bytes back.
            // The engine dedups chunks already present remotely during this push.
            session.engine.modify_binary(file_id.to_owned(), bytes).map_err(|e| e.to_string())?;
            session.engine.sync(file_id.to_owned()).map_err(|e| e.to_string())?;
            reassemble_to_disk(session, file_id, path)?;
            if let Ok(converged) = session.engine.get_manifest(file_id.to_owned()) {
                baselines.set(file_id.to_owned(), converged);
            }
        }
        FileAction::Conflict => return Ok(FileAction::Conflict),
    }
    Ok(action)
}

/// Reassemble a file's converged content from the remote and write it to
/// `dest`, but only when the bytes differ from what is already on disk. The
/// engine resolves the manifest through the union pack index, range-reads each
/// chunk from its pack, and verifies each chunk's hash on read (so a truncated
/// or corrupted transfer fails loud rather than writing wrong bytes).
fn reassemble_to_disk(session: &Session, file_id: &str, dest: &Path) -> Result<(), String> {
    let content = session.engine.read_binary(file_id.to_owned()).map_err(|e| e.to_string())?;
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
#[allow(dead_code)]
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

/// List the version history (oplog) of the active item's file into the daemon
/// log, with a one-line count on the status bar. Emits no `result` payload.
async fn list_history(ctx: &ActionContext) -> Result<RunOutput, String> {
    let file_id = active_file_id(ctx).await?;
    let session = open_session_notified(ctx, BinaryConflictPolicy::Manual).await?;
    let mut items = session
        .remote
        .list_oplogs(file_id.clone())
        .map_err(|e| format!("failed to list history: {e}"))?;
    items.sort_by(|a, b| a.sequence.cmp(&b.sequence).then(a.client_id.cmp(&b.client_id)));
    let mut rows = Vec::with_capacity(items.len());
    for item in &items {
        let stamp = match session.remote.get_oplog(file_id.clone(), item.remote_path.clone()) {
            Ok(bytes) => serde_json::from_slice::<OpLogEntry>(&bytes)
                .map(|e| format_stamp(e.timestamp))
                .unwrap_or_else(|_| "?".to_owned()),
            Err(_) => "?".to_owned(),
        };
        rows.push(serde_json::json!({
            "sequence": item.sequence.to_string(),
            "timestamp": stamp,
            "client": item.client_id,
            "file": file_id,
        }));
    }
    set_status(ctx, &format!("{} version(s) for {file_id}", items.len()), NotifyKind::Info).await;
    Ok(RunOutput::ok(serde_json::json!({ "history_pane": rows }).to_string())
        .content_type(UI_JSON))
}

/// Roll the active item's file back to a chosen sequence.
async fn rollback(ctx: &ActionContext) -> Result<(), String> {
    let seq: u64 = ctx
        .params
        .get("sequence")
        .and_then(|s| s.trim().parse().ok())
        .ok_or("provide a numeric `sequence` to roll back to")?;
    let file_id = active_file_id(ctx).await?;
    let session = open_session_notified(ctx, BinaryConflictPolicy::Manual).await?;
    // Make sure local engine state reflects the remote before rolling back.
    session
        .engine
        .sync(file_id.clone())
        .map_err(|e| format!("sync before rollback failed: {e}"))?;
    let new_seq = session
        .engine
        .rollback(file_id.clone(), seq)
        .map_err(|e| format!("rollback failed: {e}"))?;
    // Reassemble the rolled-back content to disk.
    let (Some(item_id), rel) = split_file_id(&file_id) else {
        return Err("rollback: could not resolve item from file id".to_owned());
    };
    if let Ok(item) = ctx.client.get_item(item_id).await {
        let dest = abs_path(&session.library_root, &item.object_path, rel);
        reassemble_to_disk(&session, &file_id, &dest)
            .map_err(|e| format!("rollback wrote no file: {e}"))?;
    }
    let msg = format!("Rolled back to seq {seq} (new version seq {new_seq})");
    let _ = ctx.client.notify(PLUGIN_NAME, &msg, NotifyKind::Success).await;
    report(ctx, &msg, NotifyKind::Success).await;
    Ok(())
}

/// List items currently flagged as conflicted (via the indexed extra field)
/// into the log, with a count on the status bar. Emits no `result` payload.
#[allow(dead_code)]
async fn legacy_list_conflicts(ctx: &ActionContext) -> Result<(), String> {
    let items = ctx
        .client
        .list_items()
        .await
        .map_err(|e| format!("failed to list items: {e}"))?;
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
        report(ctx, "No conflicts.", NotifyKind::Success).await;
    } else {
        log(ctx, LogLevel::Info, &format!("Conflicted items:\n  {}", conflicted.join("\n  "))).await;
        set_status(ctx, &format!("{} conflicted item(s)", conflicted.len()), NotifyKind::Info).await;
    }
    Ok(())
}

/// Re-sync all conflicted items under the chosen policy.
#[allow(dead_code)]
async fn legacy_resolve_conflicts(ctx: &ActionContext) -> Result<(), String> {
    let policy = match ctx.params.get("policy").map(String::as_str) {
        Some("keep_local") => BinaryConflictPolicy::KeepLocal,
        Some("keep_remote") => BinaryConflictPolicy::KeepRemote,
        _ => BinaryConflictPolicy::KeepBoth,
    };
    let items = ctx
        .client
        .list_items()
        .await
        .map_err(|e| format!("failed to list items: {e}"))?;
    let conflicted: Vec<String> = items
        .into_iter()
        .filter(|item| {
            item.extra.get(NS).and_then(|ns| ns.get("status")).map(|s| s == "conflict").unwrap_or(false)
        })
        .map(|item| item.id)
        .collect();
    if conflicted.is_empty() {
        report(ctx, "No conflicts to resolve.", NotifyKind::Success).await;
        return Ok(());
    }
    let session = open_session_notified(ctx, policy).await?;
    let mut baselines = baseline::Baselines::load(&session.plugin_dir);
    let mut state = SyncState::load(&session.plugin_dir);
    let mut resolved = 0usize;
    for item_id in &conflicted {
        if sync_one_item(ctx, &session, item_id, &mut baselines, &mut state).await.is_ok() {
            resolved += 1;
        }
    }
    if let Err(e) = baselines.save(&session.plugin_dir) {
        log(ctx, LogLevel::Warn, &format!("could not persist sync baselines: {e}")).await;
    }
    let _ = state.save(&session.plugin_dir);
    let msg = format!("Resolved {resolved}/{} conflicted item(s)", conflicted.len());
    let _ = ctx.client.notify(PLUGIN_NAME, &msg, NotifyKind::Success).await;
    report(ctx, &msg, NotifyKind::Success).await;
    Ok(())
}

/// Return durable file-level conflict records for the schema-v2 table.
async fn list_conflicts_v2(ctx: &ActionContext) -> Result<RunOutput, String> {
    let session = open_session_notified(ctx, BinaryConflictPolicy::Manual).await?;
    let state = SyncState::load(&session.plugin_dir);
    let items = ctx.client.list_items().await.map_err(|e| format!("failed to list items: {e}"))?;
    let titles: std::collections::HashMap<String, String> = items
        .into_iter()
        .map(|item| (item.id, item.title))
        .collect();
    let rows: Vec<serde_json::Value> = state
        .conflicts()
        .map(|record| serde_json::json!({
            "conflict_id": record.id,
            "item": titles.get(&record.item_id).cloned().unwrap_or_else(|| record.item_id.clone()),
            "file": record.relative_path,
            "detected": format_stamp(record.detected_at_ms),
            "local_chunks": record.local_manifest.len().to_string(),
            "remote_chunks": record.remote_manifest.len().to_string(),
        }))
        .collect();
    set_status(ctx, &format!("{} file conflict(s) require review", rows.len()), NotifyKind::Info).await;
    Ok(RunOutput::ok(serde_json::json!({ "conflict_pane": rows }).to_string())
        .content_type(UI_JSON))
}

/// Resolve one selected conflict, queue its item, then force a sync.
async fn resolve_conflict_v2(ctx: &ActionContext) -> Result<(), String> {
    let conflict_id = ctx.params.get("conflict_id").ok_or("select a conflict to resolve")?;
    let policy = ctx.params.get("policy").map(String::as_str).unwrap_or("keep_both");
    let session = open_session_notified(ctx, BinaryConflictPolicy::Manual).await?;
    let mut state = SyncState::load(&session.plugin_dir);
    let record = state.get(conflict_id).cloned().ok_or("the selected conflict is no longer pending")?;
    let item = ctx.client.get_item(&record.item_id).await.map_err(|e| e.to_string())?;
    let path = abs_path(&session.library_root, &item.object_path, &record.relative_path);
    let local = std::fs::read(&path).map_err(|e| format!("cannot read local conflict copy: {e}"))?;
    if binary::manifest(&local) != record.local_manifest {
        return Err("local file changed after conflict detection; refresh and review it again".to_owned());
    }
    session.engine.sync(record.file_id.clone()).map_err(|e| e.to_string())?;
    match policy {
        "keep_local" => {
            session.engine.modify_binary(record.file_id.clone(), local).map_err(|e| e.to_string())?;
            reassemble_to_disk(&session, &record.file_id, &path)?;
        }
        "keep_remote" => reassemble_to_disk(&session, &record.file_id, &path)?,
        "keep_both" => {
            let temp = std::env::temp_dir().join("localref-s3sync").join(&record.relative_path);
            if let Some(parent) = temp.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&temp, &local).map_err(|e| e.to_string())?;
            reassemble_to_disk(&session, &record.file_id, &path)?;
            let temp_path = temp.to_string_lossy().into_owned();
            ctx.client.add_file(&record.item_id, &temp_path).await.map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(temp);
        }
        _ => return Err("unknown conflict resolution policy".to_owned()),
    }
    let record = state.resolve(conflict_id).ok_or("the selected conflict is no longer pending")?;
    state.save(&session.plugin_dir)?;
    let mut baselines = baseline::Baselines::load(&session.plugin_dir);
    sync_one_item(ctx, &session, &record.item_id, &mut baselines, &mut state).await?;
    baselines.save(&session.plugin_dir)?;
    state.complete_item(&record.item_id);
    state.save(&session.plugin_dir)?;
    let msg = format!("Resolved {} and forced a sync for its item", record.relative_path);
    let _ = ctx.client.notify(PLUGIN_NAME, &msg, NotifyKind::Success).await;
    report(ctx, &msg, NotifyKind::Success).await;
    Ok(())
}

/// Validate the config end-to-end without mutating remote state: load + parse,
/// assemble the backend client (proxy/credentials), then a read-only `list`
/// probe under the prefix to confirm the backend is actually reachable. On any
/// failure, surface it as a notification too (the action output isn't always
/// visible). This is the primary way a user confirms a config works before a
/// real sync.
async fn check_config(ctx: &ActionContext) -> Result<(), String> {
    match check_config_inner() {
        Ok(summary) => {
            // Success is worth a notification too — it's the explicit "did my
            // config work?" check — plus the log/status line.
            let _ = ctx.client.notify(PLUGIN_NAME, &summary, NotifyKind::Success).await;
            report(ctx, &summary, NotifyKind::Success).await;
            Ok(())
        }
        Err(e) => {
            let _ = ctx.client.notify(PLUGIN_NAME, &e, NotifyKind::Error).await;
            Err(e)
        }
    }
}

/// The steps of [`check_config`]; each `?` names the failing stage in its error.
fn check_config_inner() -> Result<String, String> {
    let lr = LocalrefConfig::load()?;
    let library_root = lr.library_root().to_path_buf();
    let cfg = S3SyncConfig::load(&library_root)?;

    let store = build_object_store(&cfg).map_err(|e| format!("building client: {e}"))?;
    let remote = S3Remote::new(store, cfg.prefix.clone(), Handle::current());

    // A `list` needs no pre-existing objects and writes nothing, so it is safe
    // against a real bucket/WebDAV path and proves credentials + network + proxy.
    let packs = remote
        .list_packs()
        .map_err(|e| format!("reaching the backend: {e}"))?;

    let target = match cfg.backend {
        Backend::S3 => format!("bucket={}", cfg.bucket),
        Backend::Http => {
            format!("url={}", cfg.http.as_ref().map(|h| h.url.as_str()).unwrap_or(""))
        }
    };
    Ok(format!(
        "Config OK — backend={:?}, {target}, reachable ({} pack(s) under prefix)",
        cfg.backend,
        packs.len()
    ))
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
