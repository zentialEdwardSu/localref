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
mod local_replica;
mod s3_remote;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use config::{Backend, LogDetail, S3SyncConfig};
use listener::RuntimeEventBuffer;
use local_replica::LocalrefReplica;
use localref_core::config::LocalrefConfig;
use localref_plugin_sdk::{
    ActionContext, Invocation, LocalrefClient, LogLevel, NotifyKind,
    RunOutput, emit, parse_args,
};
use object_store::aws::{AmazonS3Builder, S3ConditionalPut};
use object_store::http::HttpBuilder;
use object_store::local::LocalFileSystem;
use object_store::{ClientOptions, ObjectStore};
use rollforward::{
    ConflictQuery, MaintenanceRequest, PreservedVersion, RedbRuntimeStore,
    RemoteStorageV2, ReplicaState, ResolveConflictRequest, ResourceKey,
    RollbackRequest, SyncRequest, SyncRuntime, VersionChoice,
};
use s3_remote::S3Remote;
use tokio::runtime::Handle;

/// Plugin name (log target and notification title).
const PLUGIN_NAME: &str = "s3sync";
/// Extra namespace for this plugin's per-item state.
const NS: &str = "s3sync";
/// Row color used to flag a conflicted item in the desktop list.
const CONFLICT_COLOR: &str = "#e11d48";
/// Structured preview payload consumed by schema-v2 plugin display panes.
const UI_JSON: &str = "application/vnd.localref.plugin-ui+json;v=1";
static LOG_DETAIL: AtomicU8 = AtomicU8::new(2);

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let Some(invocation) = parse_args(std::env::args().skip(1)) else {
        emit(&RunOutput::error("usage: s3sync <run|cron> … --endpoint …"));
        return;
    };
    match invocation {
        Invocation::Manifest => {
            println!(
                "s3sync — sync item files to S3 with history and conflict resolution"
            );
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
        Invocation::Cron { job, endpoint } => {
            let output = if job == "nightly_sync" {
                let client = LocalrefClient::new(&endpoint);
                let ctx = ActionContext {
                    selected: vec![],
                    active: None,
                    params: Default::default(),
                    client,
                };
                // Cron runs unattended, so surface the outcome as a notification.
                let (out, title, body, kind) = match sync_all(&ctx).await {
                    Ok(msg) => (
                        RunOutput::done(),
                        "s3sync nightly sync",
                        msg,
                        NotifyKind::Success,
                    ),
                    Err(msg) => (
                        RunOutput::error(msg.clone()),
                        "s3sync nightly sync failed",
                        msg,
                        NotifyKind::Error,
                    ),
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

async fn log_event(
    ctx: &ActionContext,
    level: LogLevel,
    event_kind: &str,
    item_id: Option<&str>,
    path: Option<&str>,
    message: &str,
) {
    let detail = LOG_DETAIL.load(Ordering::Relaxed);
    if (detail == 0
        && !(event_kind.starts_with("s3sync.run.")
            || event_kind.starts_with("s3sync.item.")
            || event_kind == "s3sync.conflict"
            || event_kind == "s3sync.truncate"
            || event_kind == "s3sync.gc"))
        || (detail == 1 && event_kind.contains(".pack_"))
    {
        return;
    }
    let _ = ctx
        .client
        .log_with(PLUGIN_NAME, level, message, Some(event_kind), item_id, path)
        .await;
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
        "sync_selected" => {
            sync_items_v2(ctx, &target_ids(ctx)).await.map(drop)
        }
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
#[derive(Clone)]
struct Session {
    runtime: Arc<SyncRuntime>,
    remote: Arc<S3Remote>,
    runtime_events: Arc<RuntimeEventBuffer>,
    /// Absolute library root, for resolving item file paths.
    library_root: PathBuf,
    /// Plugin state dir (`<library>/.localref/s3sync`), home of the baseline store.
    plugin_dir: PathBuf,
    history_retention_versions: u64,
    trash_retention_days: u64,
}

/// Open a session, and if it fails — most commonly a missing or invalid
/// config — surface the error as a daemon notification before returning it.
/// Action output isn't always visible to the user (e.g. a triggered run), so
/// the notification is what makes a setup problem noticeable.
async fn open_session_notified(
    ctx: &ActionContext,
) -> Result<Session, String> {
    match open_session(ctx) {
        Ok(session) => Ok(session),
        Err(e) => {
            let _ =
                ctx.client.notify(PLUGIN_NAME, &e, NotifyKind::Error).await;
            Err(e)
        }
    }
}

/// Build an engine session from the plugin + library config.
///
/// `policy` selects the binary conflict resolution for this invocation.
fn open_session(ctx: &ActionContext) -> Result<Session, String> {
    let lr = LocalrefConfig::load()?;
    let library_root = lr.library_root().to_path_buf();
    let cfg = S3SyncConfig::load(&library_root)?;
    LOG_DETAIL.store(
        match cfg.log_detail {
            LogDetail::Summary => 0,
            LogDetail::Files => 1,
            LogDetail::FilesAndPacks => 2,
        },
        Ordering::Relaxed,
    );
    let plugin_dir = config::plugin_dir(&library_root);

    let store: Arc<dyn ObjectStore> = build_object_store(&cfg)?;
    let handle = Handle::current();
    let remote = Arc::new(S3Remote::new_with_concurrency(
        store,
        cfg.prefix.clone(),
        handle,
        cfg.pack_upload_concurrency,
    ));

    let runtime_store_path = plugin_dir.join("store-v2.redb");
    if let Some(parent) = runtime_store_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let runtime_store = Arc::new(
        RedbRuntimeStore::open(&runtime_store_path)
            .map_err(|e| e.to_string())?,
    );
    let runtime_events = Arc::new(RuntimeEventBuffer::default());
    let replica = Arc::new(LocalrefReplica::new(
        ctx.client.clone(),
        Handle::current(),
        library_root.clone(),
        plugin_dir.join("tmp"),
    ));
    let runtime = Arc::new(
        SyncRuntime::with_backends(
            cfg.client_id.clone(),
            runtime_store,
            remote.clone(),
            replica,
            runtime_events.clone(),
        )
        .map_err(|e| e.to_string())?,
    );
    Ok(Session {
        runtime,
        remote,
        runtime_events,
        library_root,
        plugin_dir,
        history_retention_versions: cfg.history_retention_versions,
        trash_retention_days: cfg.trash_retention_days,
    })
}

/// Construct the object store for the configured backend.
fn build_object_store(
    cfg: &S3SyncConfig,
) -> Result<Arc<dyn ObjectStore>, String> {
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
        return Ok(Arc::new(
            LocalFileSystem::new_with_prefix(local)
                .map_err(|e| e.to_string())?,
        ));
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
fn build_http_store(
    cfg: &S3SyncConfig,
) -> Result<Arc<dyn ObjectStore>, String> {
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

/// Sync every item in the library. Returns the human summary on success.
async fn sync_all(ctx: &ActionContext) -> Result<String, String> {
    let items = ctx
        .client
        .list_items()
        .await
        .map_err(|e| format!("failed to list items: {e}"))?;
    let ids: Vec<String> = items.into_iter().map(|item| item.id).collect();
    sync_items_v2(ctx, &ids).await
}

/// Run the complete rollforward v2 reconciliation for the selected scopes.
async fn sync_items_v2(
    ctx: &ActionContext,
    item_ids: &[String],
) -> Result<String, String> {
    if item_ids.is_empty() {
        return Err("no items to sync".to_owned());
    }
    let session = open_session_notified(ctx).await?;
    let run_id = format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis())
    );
    log_event(
        ctx,
        LogLevel::Info,
        "s3sync.run.start",
        None,
        None,
        &format!("run={run_id} engine=v2 scopes={}", item_ids.len()),
    )
    .await;
    set_status(
        ctx,
        &format!("Reconciling {} item(s)", item_ids.len()),
        NotifyKind::Info,
    )
    .await;

    let runtime = session.runtime.clone();
    let request = SyncRequest { scopes: item_ids.to_vec() };
    let report =
        tokio::task::spawn_blocking(move || runtime.reconcile(request))
            .await
            .map_err(|error| format!("v2 sync worker failed: {error}"))?
            .map_err(|error| error.to_string())?;

    save_conflict_mirror(
        &session.plugin_dir,
        &session.runtime.list_conflicts(ConflictQuery::default()),
    )?;

    for event in session.runtime_events.take() {
        let item = event.resource.as_ref().map(|key| key.scope_id.as_str());
        let path = event.resource.as_ref().map(|key| key.resource_id.as_str());
        let detail_json = if LOG_DETAIL.load(Ordering::Relaxed) == 1
            && event.stage == "download.plan"
        {
            serde_json::from_str::<serde_json::Value>(&event.detail_json)
                .map(|mut value| {
                    if let Some(object) = value.as_object_mut()
                        && let Some(packs) = object.remove("packs")
                    {
                        object.insert(
                            "pack_count".into(),
                            serde_json::json!(
                                packs.as_array().map_or(0, Vec::len)
                            ),
                        );
                    }
                    value.to_string()
                })
                .unwrap_or_else(|_| event.detail_json.clone())
        } else {
            event.detail_json.clone()
        };
        log_event(
            ctx,
            if event.stage == "error" {
                LogLevel::Warn
            } else {
                LogLevel::Info
            },
            &format!("s3sync.{}", event.stage),
            item,
            path,
            &format!(
                "run={} operation={} {}",
                event.run_id, event.operation_id, detail_json
            ),
        )
        .await;
    }
    for stat in session.remote.take_upload_stats() {
        log_event(
            ctx,
            LogLevel::Info,
            "s3sync.upload.pack_complete",
            None,
            None,
            &format!("run={run_id} {}", stat.log_message()),
        )
        .await;
    }

    for scope in &report.scopes {
        let blocked = matches!(
            scope.status,
            rollforward::ScopeStatus::Partial
                | rollforward::ScopeStatus::Blocked
                | rollforward::ScopeStatus::Failed
        );
        let _ = ctx
            .client
            .set_item_extra(
                &scope.scope_id,
                NS,
                "status",
                Some(if blocked { "blocked" } else { "synced" }),
            )
            .await;
        let _ = ctx
            .client
            .set_bar_color(&scope.scope_id, blocked.then_some(CONFLICT_COLOR))
            .await;
        log_event(
            ctx,
            if blocked { LogLevel::Warn } else { LogLevel::Info },
            "s3sync.item.complete",
            Some(&scope.scope_id),
            None,
            &format!(
                "run={run_id} status={:?} conflicts={} failures={}",
                scope.status, scope.conflicts, scope.failures
            ),
        )
        .await;
    }

    if session.history_retention_versions > 0 {
        let runtime = session.runtime.clone();
        let maintenance = MaintenanceRequest {
            scopes: item_ids.to_vec(),
            history_retention_versions: session.history_retention_versions,
        };
        let maintenance =
            tokio::task::spawn_blocking(move || runtime.maintain(maintenance))
                .await
                .map_err(|error| {
                    format!("v2 maintenance worker failed: {error}")
                })?
                .map_err(|error| error.to_string())?;
        log_event(
            ctx,
            LogLevel::Info,
            "s3sync.gc",
            None,
            None,
            &format!(
                "run={run_id} resources={} commits_deleted={} deferred={} packs_deleted={} packs_repacked={} bytes_reclaimed={}",
                maintenance.resources,
                maintenance.commits_deleted,
                maintenance.deferred,
                maintenance.packs_deleted,
                maintenance.packs_repacked,
                maintenance.bytes_reclaimed
            ),
        )
        .await;
    }
    let (trash_files, trash_bytes) = cleanup_trash(&session)?;
    let summary = format!(
        "Synced {} item(s): {} uploaded, {} downloaded, {} remote delete(s), {} local archive(s), {} unchanged, {} conflict(s), {} failure(s)",
        item_ids.len(),
        report.uploaded,
        report.downloaded,
        report.deleted_remote,
        report.deleted_local,
        report.unchanged,
        report.conflicts,
        report.failures
    );
    log_event(
        ctx,
        LogLevel::Info,
        "s3sync.run.complete",
        None,
        None,
        &format!(
            "run={run_id} trash_removed_files={trash_files} trash_removed_bytes={trash_bytes} {summary}"
        ),
    )
    .await;
    set_status(
        ctx,
        &summary,
        if report.failures > 0 {
            NotifyKind::Error
        } else if report.conflicts > 0 {
            NotifyKind::Info
        } else {
            NotifyKind::Success
        },
    )
    .await;
    if report.failures > 0 { Err(summary) } else { Ok(summary) }
}

const CONFLICT_MIRROR: &str = "conflicts-v2.json";

fn save_conflict_mirror(
    plugin_dir: &Path,
    conflicts: &[rollforward::ConflictRecord],
) -> Result<(), String> {
    std::fs::create_dir_all(plugin_dir).map_err(|error| error.to_string())?;
    let target = plugin_dir.join(CONFLICT_MIRROR);
    let temporary = plugin_dir.join(format!("{CONFLICT_MIRROR}.tmp"));
    let bytes =
        serde_json::to_vec(conflicts).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    if target.exists() {
        std::fs::remove_file(&target).map_err(|error| error.to_string())?;
    }
    std::fs::rename(temporary, target).map_err(|error| error.to_string())
}

fn load_conflict_mirror(
    plugin_dir: &Path,
) -> Vec<rollforward::ConflictRecord> {
    std::fs::read(plugin_dir.join(CONFLICT_MIRROR))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Resolve the v2 resource key for the active item's main file, or its first
/// regular attachment when no main file is set.
async fn active_resource_key(
    ctx: &ActionContext,
) -> Result<ResourceKey, String> {
    let item_id = ctx.active.clone().ok_or("no active item")?;
    let item =
        ctx.client.get_item(&item_id).await.map_err(|e| e.to_string())?;
    let rel = if let Some(main) = item.main_file {
        main
    } else {
        let files = ctx
            .client
            .item_files(&item_id)
            .await
            .map_err(|e| e.to_string())?;
        files
            .files
            .into_iter()
            .find(|f| f.kind == "file")
            .map(|f| f.path)
            .ok_or("item has no files to show history for")?
    };
    Ok(ResourceKey::new(item_id, rel))
}

/// List the version history (oplog) of the active item's file into the daemon
/// log, with a one-line count on the status bar. Emits no `result` payload.
async fn list_history(ctx: &ActionContext) -> Result<RunOutput, String> {
    let resource = active_resource_key(ctx).await?;
    let session = open_session_notified(ctx).await?;
    let history = session.runtime.history(resource.clone());
    let rows: Vec<_> = history
        .iter()
        .rev()
        .map(|version| {
            serde_json::json!({
                // Keep the schema field name for the existing pane; v2 values are
                // immutable commit IDs rather than linear sequence numbers.
                "sequence": version.commit_id,
                "commit_id": version.commit_id,
                "timestamp": format_stamp(version.timestamp),
                "client": version.author,
                "file": resource.resource_id,
                "deleted": version.deleted,
            })
        })
        .collect();
    set_status(
        ctx,
        &format!("{} version(s) for {}", rows.len(), resource.resource_id),
        NotifyKind::Info,
    )
    .await;
    Ok(RunOutput::ok(serde_json::json!({ "history_pane": rows }).to_string())
        .content_type(UI_JSON))
}

/// Roll the active item's file back to an immutable v2 commit.
async fn rollback(ctx: &ActionContext) -> Result<(), String> {
    let commit_id = ctx
        .params
        .get("commit_id")
        .or_else(|| ctx.params.get("sequence"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or("select a version to roll back to")?
        .to_owned();
    let resource = active_resource_key(ctx).await?;
    let session = open_session_notified(ctx).await?;
    let runtime = session.runtime.clone();
    let rollback_resource = resource.clone();
    let rollback_commit = commit_id.clone();
    tokio::task::spawn_blocking(move || {
        runtime.rollback(RollbackRequest {
            resource: rollback_resource,
            commit_id: rollback_commit,
        })
    })
    .await
    .map_err(|error| format!("rollback worker failed: {error}"))?
    .map_err(|error| format!("rollback failed: {error}"))?;
    save_conflict_mirror(
        &session.plugin_dir,
        &session.runtime.list_conflicts(ConflictQuery::default()),
    )?;
    let msg = format!(
        "Rolled back {} to commit {}",
        resource.resource_id,
        &commit_id[..commit_id.len().min(12)]
    );
    let _ = ctx.client.notify(PLUGIN_NAME, &msg, NotifyKind::Success).await;
    report(ctx, &msg, NotifyKind::Success).await;
    Ok(())
}

/// Return durable file-level conflict records for the schema-v2 table.
async fn list_conflicts_v2(ctx: &ActionContext) -> Result<RunOutput, String> {
    // A sync process owns the v2 redb writer. The engine publishes this
    // read-only mirror after every run so rendering the page never opens a
    // second database writer; resolution still validates authoritative state.
    let library_root = LocalrefConfig::load()?.library_root().to_path_buf();
    let conflicts = load_conflict_mirror(&config::plugin_dir(&library_root));
    let items = ctx
        .client
        .list_items()
        .await
        .map_err(|e| format!("failed to list items: {e}"))?;
    let titles: std::collections::HashMap<String, String> =
        items.into_iter().map(|item| (item.id, item.title)).collect();
    let rows: Vec<serde_json::Value> = conflicts
        .iter()
        .map(|record| serde_json::json!({
            "conflict_id": record.id,
            "item": titles.get(&record.resource.scope_id).cloned().unwrap_or_else(|| record.resource.scope_id.clone()),
            "file": record.resource.resource_id,
            "detected": format_stamp(record.created_at),
            "local_chunks": record.local.content_id().unwrap_or("-").chars().take(12).collect::<String>(),
            "remote_chunks": record.remote_heads.len().to_string(),
            "remote_head": record.remote_heads.join(","),
            "conflict_type": format!("{:?}", record.kind),
        }))
        .collect();
    set_status(
        ctx,
        &format!("{} file conflict(s) require review", rows.len()),
        NotifyKind::Info,
    )
    .await;
    Ok(RunOutput::ok(serde_json::json!({ "conflict_pane": rows }).to_string())
        .content_type(UI_JSON))
}

/// Resolve one selected v2 conflict with optimistic revalidation in the engine.
async fn resolve_conflict_v2(ctx: &ActionContext) -> Result<(), String> {
    let conflict_id =
        ctx.params.get("conflict_id").ok_or("select a conflict to resolve")?;
    let policy =
        ctx.params.get("policy").map(String::as_str).unwrap_or("keep_both");
    let session = open_session_notified(ctx).await?;
    let record = session
        .runtime
        .list_conflicts(ConflictQuery::default())
        .into_iter()
        .find(|record| record.id == *conflict_id)
        .ok_or("the selected conflict is no longer pending")?;
    let selected_remote = || -> Result<String, String> {
        if let Some(commit) = ctx
            .params
            .get("remote_commit")
            .map(|commit| commit.trim())
            .filter(|commit| !commit.is_empty())
        {
            if record.remote_heads.iter().any(|head| head == commit) {
                return Ok(commit.to_owned());
            }
            return Err(
                "the selected remote version is no longer a head".into()
            );
        }
        match record.remote_heads.as_slice() {
            [only] => Ok(only.clone()),
            [] => Err("the conflict has no remote version".into()),
            _ => Err("this conflict has multiple remote heads; select a remote version first".into()),
        }
    };
    let (primary, preserved) = match policy {
        "keep_local" => {
            let choice =
                if matches!(record.local, ReplicaState::Present { .. }) {
                    VersionChoice::Local
                } else {
                    VersionChoice::Delete
                };
            (choice, Vec::new())
        }
        "keep_remote" => (
            VersionChoice::Remote { commit_id: selected_remote()? },
            Vec::new(),
        ),
        "keep_both"
            if matches!(record.local, ReplicaState::Present { .. }) =>
        {
            let target =
                conflict_copy_key(&record.resource, &record.id, "local");
            (
                VersionChoice::Remote { commit_id: selected_remote()? },
                vec![PreservedVersion {
                    source: VersionChoice::Local,
                    target,
                }],
            )
        }
        "keep_both" => {
            let remote = selected_remote()?;
            let target =
                conflict_copy_key(&record.resource, &record.id, "remote");
            (
                VersionChoice::Delete,
                vec![PreservedVersion {
                    source: VersionChoice::Remote { commit_id: remote },
                    target,
                }],
            )
        }
        _ => return Err("unknown conflict resolution policy".to_owned()),
    };
    let runtime = session.runtime.clone();
    let request = ResolveConflictRequest {
        conflict_id: conflict_id.clone(),
        primary,
        preserved,
    };
    tokio::task::spawn_blocking(move || runtime.resolve(request))
        .await
        .map_err(|error| {
            format!("conflict resolution worker failed: {error}")
        })?
        .map_err(|error| error.to_string())?;
    save_conflict_mirror(
        &session.plugin_dir,
        &session.runtime.list_conflicts(ConflictQuery::default()),
    )?;
    let msg = format!(
        "Resolved {} and forced a sync for its item",
        record.resource.resource_id
    );
    let _ = ctx.client.notify(PLUGIN_NAME, &msg, NotifyKind::Success).await;
    report(ctx, &msg, NotifyKind::Success).await;
    Ok(())
}

fn conflict_copy_key(
    original: &ResourceKey,
    conflict_id: &str,
    side: &str,
) -> ResourceKey {
    let path = Path::new(&original.resource_id);
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
    let stem = path
        .file_stem()
        .and_then(|part| part.to_str())
        .unwrap_or("attachment");
    let extension = path.extension().and_then(|part| part.to_str());
    let short = &conflict_id[..conflict_id.len().min(8)];
    let mut leaf = format!("{stem}.{side}-conflict-{short}");
    if let Some(extension) = extension {
        leaf.push('.');
        leaf.push_str(extension);
    }
    let relative = parent
        .map_or_else(|| PathBuf::from(&leaf), |parent| parent.join(&leaf));
    ResourceKey::new(
        original.scope_id.clone(),
        relative.to_string_lossy().replace('\\', "/"),
    )
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
            let _ = ctx
                .client
                .notify(PLUGIN_NAME, &summary, NotifyKind::Success)
                .await;
            report(ctx, &summary, NotifyKind::Success).await;
            Ok(())
        }
        Err(e) => {
            let _ =
                ctx.client.notify(PLUGIN_NAME, &e, NotifyKind::Error).await;
            Err(e)
        }
    }
}

/// The steps of [`check_config`]; each `?` names the failing stage in its error.
fn check_config_inner() -> Result<String, String> {
    let lr = LocalrefConfig::load()?;
    let library_root = lr.library_root().to_path_buf();
    let cfg = S3SyncConfig::load(&library_root)?;

    let store = build_object_store(&cfg)
        .map_err(|e| format!("building client: {e}"))?;
    let remote = S3Remote::new_with_concurrency(
        store,
        cfg.prefix.clone(),
        Handle::current(),
        cfg.pack_upload_concurrency,
    );

    // A `list` needs no pre-existing objects and writes nothing, so it is safe
    // against a real bucket/WebDAV path and proves credentials + network + proxy.
    let packs = remote
        .list_pack_ids()
        .map_err(|e| format!("reaching the backend: {e}"))?;

    let target = match cfg.backend {
        Backend::S3 => format!("bucket={}", cfg.bucket),
        Backend::Http => {
            format!(
                "url={}",
                cfg.http.as_ref().map(|h| h.url.as_str()).unwrap_or("")
            )
        }
    };
    Ok(format!(
        "Config OK — backend={:?}, {target}, reachable ({} pack(s) under prefix)",
        cfg.backend,
        packs.len()
    ))
}

fn cleanup_trash(session: &Session) -> Result<(u64, u64), String> {
    if session.trash_retention_days == 0 {
        return Ok((0, 0));
    }
    let root = session.library_root.join(".localref").join("trash").join(NS);
    if !root.is_dir() {
        return Ok((0, 0));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let max_age = u128::from(session.trash_retention_days) * 86_400_000;
    let mut files = 0;
    let mut bytes = 0;
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some(stamp) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u128>().ok())
        else {
            continue;
        };
        if now.saturating_sub(stamp) <= max_age {
            continue;
        }
        count_tree(&entry.path(), &mut files, &mut bytes)?;
        std::fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
    }
    Ok((files, bytes))
}

fn count_tree(
    path: &Path,
    files: &mut u64,
    bytes: &mut u64,
) -> Result<(), String> {
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        if metadata.is_dir() {
            count_tree(&entry.path(), files, bytes)?;
        } else if metadata.is_file() {
            *files += 1;
            *bytes += metadata.len();
        }
    }
    Ok(())
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

#[cfg(test)]
mod v2_tests {
    use super::*;
    use rollforward::{ConflictKind, RemoteResourceState};

    #[test]
    fn conflict_mirror_is_readable_without_opening_runtime_store() {
        let dir = tempfile::TempDir::new().unwrap();
        let key = ResourceKey::new("item", "nested/file.pdf");
        let conflict = rollforward::ConflictRecord::new(
            key,
            ConflictKind::InitialDivergence,
            &ReplicaState::present("local", 5, "version"),
            &RemoteResourceState::Present {
                content_id: "remote".into(),
                size: 6,
                heads: vec!["head".into()],
            },
        );
        save_conflict_mirror(dir.path(), std::slice::from_ref(&conflict))
            .unwrap();
        assert_eq!(load_conflict_mirror(dir.path()), vec![conflict]);
    }

    #[test]
    fn conflict_copy_preserves_parent_and_extension() {
        let original = ResourceKey::new("item", "figures/chart.png");
        let copy = conflict_copy_key(&original, "0123456789", "local");
        assert_eq!(
            copy,
            ResourceKey::new(
                "item",
                "figures/chart.local-conflict-01234567.png"
            )
        );
    }
}
