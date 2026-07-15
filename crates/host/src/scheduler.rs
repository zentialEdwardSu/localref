//! Background plugin workers: the hook dispatcher and the cron scheduler.
//!
//! The daemon (in `localref-core`) is synchronous and publishes a
//! [`DaemonEvent`] after each mutating action completes. This module bridges
//! that stream to the async tokio runtime: it spawns hook plugins
//! fire-and-forget when a matching event arrives, and spawns cron plugins on a
//! crontab-like schedule. Neither path can block or fail a daemon action.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use cron::Schedule;
use localref_core::schedule::ScheduledCall;
use localref_core::{DaemonEvent, LocalrefDaemon, PauseMode};
use localref_plugin::{
    ActionArgs, DiscoveredPlugin, HookArgs, InvocationKind,
    InvocationTracking, PluginProcessRegistry, invoke_action, invoke_cron,
    invoke_hook,
};
use tokio::sync::broadcast::error::RecvError;

use crate::health::RuntimeHealthTracker;

/// Shared set of disabled plugin names, kept in sync with the UI server.
type Disabled = Arc<RwLock<BTreeSet<String>>>;

/// Atomically swappable discovered-plugin list, shared with the FFI host.
///
/// A rescan replaces the inner `Arc` under the write lock; the workers read a
/// cheap snapshot (clone of the inner `Arc`) on their next iteration and so
/// observe added or removed plugins without a restart.
pub type SharedPlugins = Arc<RwLock<Arc<Vec<DiscoveredPlugin>>>>;

/// Return a cheap snapshot of the current plugin list.
fn plugins_snapshot(plugins: &SharedPlugins) -> Arc<Vec<DiscoveredPlugin>> {
    match plugins.read() {
        Ok(guard) => Arc::clone(&guard),
        Err(poisoned) => Arc::clone(&poisoned.into_inner()),
    }
}

/// Return whether a plugin name is currently disabled.
fn is_disabled(disabled: &Disabled, name: &str) -> bool {
    match disabled.read() {
        Ok(guard) => guard.contains(name),
        Err(poisoned) => poisoned.into_inner().contains(name),
    }
}

/// Spawn the hook dispatcher and cron scheduler onto the current runtime.
///
/// Call this inside the API tokio runtime; both workers run for the process
/// lifetime and stop when the daemon's event channel closes.
pub fn spawn_plugin_workers(
    daemon: &LocalrefDaemon,
    plugins: SharedPlugins,
    endpoint: String,
    disabled: Disabled,
    registry: Arc<PluginProcessRegistry>,
    health: RuntimeHealthTracker,
) {
    let snapshot = plugins_snapshot(&plugins);
    let hook_bindings: usize =
        snapshot.iter().map(|p| p.manifest.hooks.len()).sum();
    let cron_jobs: usize =
        snapshot.iter().map(|p| p.manifest.cron.len()).sum();
    tracing::info!(
        target: "localref::plugins",
        hook_bindings,
        cron_jobs,
        "starting plugin workers",
    );
    drop(tokio::spawn(supervise_hook_dispatcher(
        daemon.clone(),
        Arc::clone(&plugins),
        endpoint.clone(),
        disabled.clone(),
        Arc::clone(&registry),
        health.clone(),
    )));
    drop(tokio::spawn(supervise_cron_scheduler(
        daemon.clone(),
        plugins,
        endpoint,
        disabled,
        registry,
        health,
    )));
}

async fn supervise_hook_dispatcher(
    daemon: LocalrefDaemon,
    plugins: SharedPlugins,
    endpoint: String,
    disabled: Disabled,
    registry: Arc<PluginProcessRegistry>,
    health: RuntimeHealthTracker,
) {
    let mut failures = VecDeque::new();
    loop {
        let worker = tokio::spawn(run_hook_dispatcher(
            daemon.subscribe(),
            Arc::clone(&plugins),
            endpoint.clone(),
            disabled.clone(),
            Arc::clone(&registry),
        ));
        match worker.await {
            Ok(()) => return,
            Err(error) => {
                let Some(delay) = register_worker_failure(
                    &health,
                    "hook-dispatcher",
                    &error,
                    &mut failures,
                ) else {
                    return;
                };
                tokio::time::sleep(delay).await;
            }
        }
    }
}

async fn supervise_cron_scheduler(
    daemon: LocalrefDaemon,
    plugins: SharedPlugins,
    endpoint: String,
    disabled: Disabled,
    registry: Arc<PluginProcessRegistry>,
    health: RuntimeHealthTracker,
) {
    let mut failures = VecDeque::new();
    loop {
        let worker = tokio::spawn(run_cron_scheduler(
            daemon.clone(),
            Arc::clone(&plugins),
            endpoint.clone(),
            disabled.clone(),
            Arc::clone(&registry),
        ));
        match worker.await {
            Ok(()) => return,
            Err(error) => {
                let Some(delay) = register_worker_failure(
                    &health,
                    "cron-scheduler",
                    &error,
                    &mut failures,
                ) else {
                    return;
                };
                tokio::time::sleep(delay).await;
            }
        }
    }
}

fn register_worker_failure(
    health: &RuntimeHealthTracker,
    component: &str,
    error: &tokio::task::JoinError,
    failures: &mut VecDeque<std::time::Instant>,
) -> Option<Duration> {
    let now = std::time::Instant::now();
    while failures.front().is_some_and(|failure| {
        now.duration_since(*failure) > Duration::from_secs(300)
    }) {
        let _ = failures.pop_front();
    }
    failures.push_back(now);
    let count = u64::try_from(failures.len()).unwrap_or(u64::MAX);
    let message = format!("worker terminated unexpectedly: {error}");
    let Some(delay) = worker_restart_delay(failures.len()) else {
        tracing::error!(target: "localref::runtime", component, count, %error, "worker entered fatal crash loop");
        health.fatal(component, message, count);
        return None;
    };
    health.degraded(component, message, count);
    tracing::warn!(target: "localref::runtime", component, count, delay = delay.as_secs(), %error, "restarting failed worker");
    Some(delay)
}

fn worker_restart_delay(failure_count: usize) -> Option<Duration> {
    [1, 5, 30]
        .get(failure_count.checked_sub(1)?)
        .copied()
        .map(Duration::from_secs)
}

fn spawn_isolated<F>(component: &'static str, future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    drop(tokio::spawn(async move {
        if let Err(error) = tokio::spawn(future).await {
            tracing::error!(target: "localref::runtime", component, %error, "isolated plugin task panicked");
        }
    }));
}

/// Consume daemon events and fan each one out to plugins bound to it.
async fn run_hook_dispatcher(
    mut rx: tokio::sync::broadcast::Receiver<DaemonEvent>,
    plugins: SharedPlugins,
    endpoint: String,
    disabled: Disabled,
    registry: Arc<PluginProcessRegistry>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                // Snapshot per event so a rescan swapping the list is picked up.
                let snapshot = plugins_snapshot(&plugins);
                dispatch_event(
                    &snapshot, &endpoint, &event, &disabled, &registry,
                )
            }
            Err(RecvError::Lagged(skipped)) => {
                tracing::warn!(
                    target: "localref::hooks",
                    skipped,
                    "hook dispatcher lagged; some events were dropped",
                );
            }
            Err(RecvError::Closed) => break,
        }
    }
}

/// Spawn every plugin bound to `event`, fire-and-forget.
fn dispatch_event(
    plugins: &[DiscoveredPlugin],
    endpoint: &str,
    event: &DaemonEvent,
    disabled: &Disabled,
    registry: &Arc<PluginProcessRegistry>,
) {
    let (item, category) = event_targets(event);
    for plugin in matching_plugins(plugins, event.event_name()) {
        if is_disabled(disabled, plugin.name()) {
            continue;
        }
        let executable = plugin.executable.clone();
        let name = plugin.name().to_string();
        let event_name = event.event_name().to_string();
        let args = HookArgs {
            endpoint: endpoint.to_string(),
            item: item.clone(),
            category: category.clone(),
        };
        let tracking = InvocationTracking {
            registry: Arc::clone(registry),
            plugin: name.clone(),
            kind: InvocationKind::Hook,
        };
        spawn_isolated("plugin-hook", async move {
            match invoke_hook(
                &executable,
                &event_name,
                &args,
                None,
                Some(tracking),
            )
            .await
            {
                Ok(output) => tracing::debug!(
                    target: "localref::hooks",
                    plugin = %name,
                    event = %event_name,
                    status = %output.status,
                    "hook ran",
                ),
                Err(error) => tracing::warn!(
                    target: "localref::hooks",
                    plugin = %name,
                    event = %event_name,
                    %error,
                    "hook failed",
                ),
            }
        });
    }
}

/// Plugins whose manifest declares a hook for this wire event name.
fn matching_plugins<'a>(
    plugins: &'a [DiscoveredPlugin],
    event_name: &str,
) -> Vec<&'a DiscoveredPlugin> {
    plugins
        .iter()
        .filter(|plugin| {
            plugin
                .manifest
                .hooks
                .iter()
                .any(|hook| hook.event.as_str() == event_name)
        })
        .collect()
}

/// The `(item, category)` ids carried by an event, for the hook argv.
fn event_targets(event: &DaemonEvent) -> (Option<String>, Option<String>) {
    match event {
        DaemonEvent::ItemImported { item_id }
        | DaemonEvent::ItemDeleted { item_id }
        | DaemonEvent::MetadataPatched { item_id }
        | DaemonEvent::ItemFileAdded { item_id } => {
            (Some(item_id.clone()), None)
        }
        DaemonEvent::CategoryChanged { item_id, category } => {
            (item_id.clone(), category.clone())
        }
        DaemonEvent::ScanCompleted { .. }
        | DaemonEvent::RulesChanged
        | DaemonEvent::SchedulesChanged
        | DaemonEvent::DaemonPaused
        | DaemonEvent::DaemonResumed
        | DaemonEvent::StatusMessage { .. } => (None, None),
    }
}

/// What a scheduled entry invokes when it fires.
enum JobKind {
    /// A manifest `[[cron]]` job: spawn its owning plugin as `cron <id>`.
    ManifestCron {
        /// Plugin executable to spawn.
        executable: PathBuf,
        /// Job id passed back as `cron <id>`.
        job_id: String,
        /// Declared subprocess timeout in seconds, if any.
        timeout_secs: Option<u64>,
    },
    /// A runtime-registered call: spawn the target plugin as `run <action>`.
    ScheduledCall {
        /// Target plugin executable to spawn.
        executable: PathBuf,
        /// Action id passed as `run <action>`.
        action: String,
        /// Parameters forwarded as `--param key=value`.
        params: Vec<(String, String)>,
    },
}

/// One scheduled entry with its parsed schedule and invocation target.
struct ScheduleEntry {
    /// Owning/target plugin name (for logging).
    plugin_name: String,
    /// Parsed cron schedule.
    schedule: Schedule,
    /// What this entry invokes when due.
    kind: JobKind,
}

/// Parse every plugin's declared cron jobs, skipping invalid expressions.
fn collect_manifest_entries(
    plugins: &[DiscoveredPlugin],
) -> Vec<ScheduleEntry> {
    let mut entries = Vec::new();
    for plugin in plugins {
        for job in &plugin.manifest.cron {
            match Schedule::from_str(&job.schedule) {
                Ok(schedule) => entries.push(ScheduleEntry {
                    plugin_name: plugin.name().to_string(),
                    schedule,
                    kind: JobKind::ManifestCron {
                        executable: plugin.executable.clone(),
                        job_id: job.id.clone(),
                        timeout_secs: job.timeout_secs,
                    },
                }),
                Err(error) => tracing::warn!(
                    target: "localref::cron",
                    plugin = %plugin.name(),
                    job = %job.id,
                    schedule = %job.schedule,
                    %error,
                    "invalid cron expression; skipping job",
                ),
            }
        }
    }
    entries
}

/// Parse runtime-registered scheduled calls, resolving each target plugin to an
/// executable and skipping unknown targets or invalid expressions.
fn collect_scheduled_call_entries(
    calls: Vec<ScheduledCall>,
    by_name: &HashMap<String, PathBuf>,
) -> Vec<ScheduleEntry> {
    let mut entries = Vec::new();
    for call in calls {
        let Some(executable) = by_name.get(&call.plugin) else {
            tracing::warn!(
                target: "localref::cron",
                schedule = %call.id,
                plugin = %call.plugin,
                "scheduled call targets an unknown plugin; skipping",
            );
            continue;
        };
        match Schedule::from_str(&call.schedule) {
            Ok(schedule) => entries.push(ScheduleEntry {
                plugin_name: call.plugin.clone(),
                schedule,
                kind: JobKind::ScheduledCall {
                    executable: executable.clone(),
                    action: call.action,
                    params: call.params.into_iter().collect(),
                },
            }),
            Err(error) => tracing::warn!(
                target: "localref::cron",
                schedule = %call.id,
                plugin = %call.plugin,
                schedule_expr = %call.schedule,
                %error,
                "invalid cron expression; skipping scheduled call",
            ),
        }
    }
    entries
}

/// Snapshot the current plugin list and build the full schedule set.
///
/// Recomputes the name-to-executable map from the snapshot so scheduled-call
/// targets resolve against the plugins present at reload time.
fn reload_schedule(
    daemon: &LocalrefDaemon,
    plugins: &SharedPlugins,
) -> Vec<ScheduleEntry> {
    let snapshot = plugins_snapshot(plugins);
    let by_name: HashMap<String, PathBuf> = snapshot
        .iter()
        .map(|plugin| (plugin.name().to_string(), plugin.executable.clone()))
        .collect();
    build_schedule(daemon, &snapshot, &by_name)
}

/// Build the full schedule set from manifest cron jobs plus runtime calls.
fn build_schedule(
    daemon: &LocalrefDaemon,
    plugins: &[DiscoveredPlugin],
    by_name: &HashMap<String, PathBuf>,
) -> Vec<ScheduleEntry> {
    let mut entries = collect_manifest_entries(plugins);
    match daemon.list_schedules() {
        Ok(calls) => {
            entries.extend(collect_scheduled_call_entries(calls, by_name));
        }
        Err(error) => tracing::warn!(
            target: "localref::cron",
            %error,
            "failed to load runtime schedules; using manifest cron jobs only",
        ),
    }
    entries
}

/// Compute the next fire time for each entry, dropping entries with none.
fn next_fire_times(
    entries: Vec<ScheduleEntry>,
) -> Vec<(ScheduleEntry, DateTime<Utc>)> {
    entries
        .into_iter()
        .filter_map(|entry| {
            let next = entry.schedule.after(&Utc::now()).next()?;
            Some((entry, next))
        })
        .collect()
}

/// Sleep until the soonest job is due, fire all due jobs, and repeat; reload
/// the whole schedule when a [`DaemonEvent::SchedulesChanged`] arrives.
///
/// No catch-up: the next fire time is always computed forward from now, so
/// jobs missed while the process was down are simply not run.
async fn run_cron_scheduler(
    daemon: LocalrefDaemon,
    plugins: SharedPlugins,
    endpoint: String,
    disabled: Disabled,
    registry: Arc<PluginProcessRegistry>,
) {
    let mut events = daemon.subscribe();
    // Rebuilt from a fresh plugin snapshot on start and on every reload, so a
    // rescan's added/removed manifest cron jobs are re-registered.
    let mut schedule = next_fire_times(reload_schedule(&daemon, &plugins));

    loop {
        // Wait for either the soonest job to be due or a reload signal. With no
        // schedules, idle until a reload arrives rather than spinning.
        let due = match schedule.iter().map(|(_, next)| *next).min() {
            Some(soonest) => {
                (soonest - Utc::now()).to_std().unwrap_or(Duration::ZERO)
            }
            None => Duration::from_secs(3600),
        };
        tokio::select! {
            () = tokio::time::sleep(due) => {}
            recv = events.recv() => {
                match recv {
                    Ok(DaemonEvent::SchedulesChanged) => {
                        schedule = next_fire_times(
                            reload_schedule(&daemon, &plugins),
                        );
                        continue;
                    }
                    // Other events don't affect the schedule set.
                    Ok(_) => continue,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                }
            }
        }

        let now = Utc::now();
        let paused = daemon.status().paused_modes.contains(&PauseMode::All);
        for (entry, next) in &mut schedule {
            if *next > now {
                continue;
            }
            if paused {
                tracing::debug!(
                    target: "localref::cron",
                    plugin = %entry.plugin_name,
                    "scheduled job skipped; daemon is paused",
                );
            } else if is_disabled(&disabled, &entry.plugin_name) {
                tracing::debug!(
                    target: "localref::cron",
                    plugin = %entry.plugin_name,
                    "scheduled job skipped; plugin is disabled",
                );
            } else {
                fire_entry(entry, &endpoint, &registry);
            }
            *next = entry
                .schedule
                .after(&now)
                .next()
                .unwrap_or_else(|| now + TimeDelta::days(3650));
        }
    }
}

/// Spawn one scheduled entry's invocation, fire-and-forget.
fn fire_entry(
    entry: &ScheduleEntry,
    endpoint: &str,
    registry: &Arc<PluginProcessRegistry>,
) {
    let name = entry.plugin_name.clone();
    let endpoint = endpoint.to_string();
    match &entry.kind {
        JobKind::ManifestCron { executable, job_id, timeout_secs } => {
            let executable = executable.clone();
            let job = job_id.clone();
            let timeout = *timeout_secs;
            let tracking = InvocationTracking {
                registry: Arc::clone(registry),
                plugin: name.clone(),
                kind: InvocationKind::Cron,
            };
            spawn_isolated("plugin-cron", async move {
                match invoke_cron(
                    &executable,
                    &job,
                    &endpoint,
                    timeout,
                    Some(tracking),
                )
                .await
                {
                    Ok(output) => tracing::debug!(
                        target: "localref::cron",
                        plugin = %name,
                        job = %job,
                        status = %output.status,
                        "cron job ran",
                    ),
                    Err(error) => tracing::warn!(
                        target: "localref::cron",
                        plugin = %name,
                        job = %job,
                        %error,
                        "cron job failed",
                    ),
                }
            });
        }
        JobKind::ScheduledCall { executable, action, params } => {
            let executable = executable.clone();
            let action = action.clone();
            let args = ActionArgs {
                endpoint,
                selected: Vec::new(),
                active: None,
                params: params.clone(),
            };
            let tracking = InvocationTracking {
                registry: Arc::clone(registry),
                plugin: name.clone(),
                kind: InvocationKind::Cron,
            };
            spawn_isolated("plugin-scheduled-action", async move {
                match invoke_action(
                    &executable,
                    &action,
                    &args,
                    None,
                    Some(tracking),
                )
                .await
                {
                    Ok(output) => tracing::debug!(
                        target: "localref::cron",
                        plugin = %name,
                        action = %action,
                        status = %output.status,
                        "scheduled call ran",
                    ),
                    Err(error) => tracing::warn!(
                        target: "localref::cron",
                        plugin = %name,
                        action = %action,
                        %error,
                        "scheduled call failed",
                    ),
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        JobKind, collect_manifest_entries, collect_scheduled_call_entries,
        event_targets, matching_plugins, spawn_isolated, worker_restart_delay,
    };
    use localref_core::DaemonEvent;
    use localref_core::schedule::ScheduledCall;
    use localref_plugin::{
        CronJob, DiscoveredPlugin, HookBinding, HookEvent, PluginManifest,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn worker_restart_policy_uses_bounded_backoff() {
        assert_eq!(worker_restart_delay(1), Some(Duration::from_secs(1)));
        assert_eq!(worker_restart_delay(2), Some(Duration::from_secs(5)));
        assert_eq!(worker_restart_delay(3), Some(Duration::from_secs(30)));
        assert_eq!(worker_restart_delay(4), None);
    }

    #[tokio::test]
    async fn isolated_plugin_panic_does_not_kill_runtime() {
        spawn_isolated("test-plugin", async {
            panic!("injected plugin panic")
        });
        tokio::task::yield_now().await;

        let next_task = tokio::spawn(async { 42_u8 }).await;

        assert_eq!(next_task.expect("runtime still executes tasks"), 42);
    }

    fn plugin(
        name: &str,
        hooks: Vec<HookEvent>,
        cron: Vec<CronJob>,
    ) -> DiscoveredPlugin {
        DiscoveredPlugin {
            dir: PathBuf::from("/plugins").join(name),
            manifest: PluginManifest {
                name: name.to_string(),
                executable: Some(name.to_string()),
                description: None,
                version: None,
                ui: None,
                hooks: hooks
                    .into_iter()
                    .map(|event| HookBinding { event })
                    .collect(),
                cron,
                extra_fields: Vec::new(),
            },
            ui: None,
            executable: PathBuf::from("/plugins").join(name).join(name),
        }
    }

    #[test]
    fn matching_plugins_selects_only_bound_plugins() {
        let plugins = vec![
            plugin("archiver", vec![HookEvent::ItemImported], vec![]),
            plugin("notifier", vec![HookEvent::ItemDeleted], vec![]),
            plugin(
                "auditor",
                vec![HookEvent::ItemImported, HookEvent::ItemDeleted],
                vec![],
            ),
        ];
        let matched = matching_plugins(&plugins, "item_imported");
        let names: Vec<&str> =
            matched.iter().map(|plugin| plugin.name()).collect();
        assert_eq!(names, vec!["archiver", "auditor"]);
    }

    #[test]
    fn event_targets_extracts_item_and_category() {
        assert_eq!(
            event_targets(&DaemonEvent::ItemImported {
                item_id: "lr:zotero:x".to_string()
            }),
            (Some("lr:zotero:x".to_string()), None),
        );
        assert_eq!(
            event_targets(&DaemonEvent::CategoryChanged {
                item_id: Some("lr:zotero:y".to_string()),
                category: Some("Inbox".to_string()),
            }),
            (Some("lr:zotero:y".to_string()), Some("Inbox".to_string())),
        );
        assert_eq!(
            event_targets(&DaemonEvent::ScanCompleted { indexed_items: 3 }),
            (None, None),
        );
    }

    #[test]
    fn collect_manifest_entries_keeps_valid_and_skips_invalid() {
        let plugins = vec![
            plugin(
                "good",
                vec![],
                vec![CronJob {
                    id: "nightly".to_string(),
                    schedule: "0 0 3 * * *".to_string(),
                    timeout_secs: None,
                }],
            ),
            plugin(
                "bad",
                vec![],
                vec![CronJob {
                    id: "broken".to_string(),
                    schedule: "not a cron expr".to_string(),
                    timeout_secs: None,
                }],
            ),
        ];
        let entries = collect_manifest_entries(&plugins);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plugin_name, "good");
        match &entries[0].kind {
            JobKind::ManifestCron { job_id, .. } => {
                assert_eq!(job_id, "nightly");
            }
            JobKind::ScheduledCall { .. } => panic!("expected manifest cron"),
        }
    }

    #[test]
    fn scheduled_calls_resolve_targets_and_skip_unknown() {
        let mut by_name = HashMap::new();
        let _ = by_name.insert(
            "archiver".to_string(),
            PathBuf::from("/plugins/archiver"),
        );
        let calls = vec![
            ScheduledCall {
                id: "weekly".to_string(),
                plugin: "archiver".to_string(),
                action: "backup".to_string(),
                params: std::collections::BTreeMap::new(),
                schedule: "0 0 3 * * *".to_string(),
            },
            // Unknown target plugin is skipped, not fatal.
            ScheduledCall {
                id: "orphan".to_string(),
                plugin: "ghost".to_string(),
                action: "noop".to_string(),
                params: std::collections::BTreeMap::new(),
                schedule: "0 0 3 * * *".to_string(),
            },
            // Invalid cron expression is skipped.
            ScheduledCall {
                id: "broken".to_string(),
                plugin: "archiver".to_string(),
                action: "backup".to_string(),
                params: std::collections::BTreeMap::new(),
                schedule: "nonsense".to_string(),
            },
        ];
        let entries = collect_scheduled_call_entries(calls, &by_name);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plugin_name, "archiver");
        match &entries[0].kind {
            JobKind::ScheduledCall { action, .. } => {
                assert_eq!(action, "backup");
            }
            JobKind::ManifestCron { .. } => panic!("expected scheduled call"),
        }
    }

    // A rescan swaps the shared list in place. This proves the cron reload path
    // reads the *current* list, not a snapshot frozen at worker-spawn time — so
    // a newly discovered plugin's manifest cron job is registered without a
    // restart (the bug that hid s3sync's nightly_sync until the daemon rebooted).
    #[test]
    fn reload_schedule_sees_swapped_in_plugin_cron() {
        use super::{SharedPlugins, reload_schedule};
        use localref_core::LocalrefDaemon;
        use std::sync::{Arc, RwLock};

        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        // Start empty: no plugins, so no manifest cron entries.
        let shared: SharedPlugins =
            Arc::new(RwLock::new(Arc::new(Vec::new())));
        assert!(reload_schedule(&daemon, &shared).is_empty());

        // Simulate a rescan discovering a plugin with a manifest cron job.
        let discovered = vec![plugin(
            "s3sync",
            vec![],
            vec![CronJob {
                id: "nightly_sync".to_string(),
                schedule: "0 0 3 * * *".to_string(),
                timeout_secs: None,
            }],
        )];
        *shared.write().unwrap() = Arc::new(discovered);

        let entries = reload_schedule(&daemon, &shared);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plugin_name, "s3sync");
        match &entries[0].kind {
            JobKind::ManifestCron { job_id, .. } => {
                assert_eq!(job_id, "nightly_sync");
            }
            JobKind::ScheduledCall { .. } => panic!("expected manifest cron"),
        }
    }
}
