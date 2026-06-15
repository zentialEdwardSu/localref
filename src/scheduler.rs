//! Background plugin workers: the hook dispatcher and the cron scheduler.
//!
//! The daemon (in `localref-core`) is synchronous and publishes a
//! [`DaemonEvent`] after each mutating action completes. This module bridges
//! that stream to the async tokio runtime: it spawns hook plugins
//! fire-and-forget when a matching event arrives, and spawns cron plugins on a
//! crontab-like schedule. Neither path can block or fail a daemon action.

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use cron::Schedule;
use localref_core::{DaemonEvent, LocalrefDaemon, PauseMode};
use localref_plugin::{
    DiscoveredPlugin, HookArgs, invoke_cron, invoke_hook,
};
use tokio::sync::broadcast::error::RecvError;

/// Spawn the hook dispatcher and cron scheduler onto the current runtime.
///
/// Call this inside the API tokio runtime; both workers run for the process
/// lifetime and stop when the daemon's event channel closes.
pub fn spawn_plugin_workers(
    daemon: &LocalrefDaemon,
    plugins: Arc<Vec<DiscoveredPlugin>>,
    endpoint: String,
) {
    let hook_bindings: usize =
        plugins.iter().map(|p| p.manifest.hooks.len()).sum();
    let cron_jobs: usize = plugins.iter().map(|p| p.manifest.cron.len()).sum();
    tracing::info!(
        target: "localref::plugins",
        hook_bindings,
        cron_jobs,
        "starting plugin workers",
    );
    let rx = daemon.subscribe();
    // Detach both workers; they run for the process lifetime.
    drop(tokio::spawn(run_hook_dispatcher(
        rx,
        Arc::clone(&plugins),
        endpoint.clone(),
    )));
    drop(tokio::spawn(run_cron_scheduler(
        daemon.clone(),
        plugins,
        endpoint,
    )));
}

/// Consume daemon events and fan each one out to plugins bound to it.
async fn run_hook_dispatcher(
    mut rx: tokio::sync::broadcast::Receiver<DaemonEvent>,
    plugins: Arc<Vec<DiscoveredPlugin>>,
    endpoint: String,
) {
    loop {
        match rx.recv().await {
            Ok(event) => dispatch_event(&plugins, &endpoint, &event),
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
) {
    let (item, category) = event_targets(event);
    for plugin in matching_plugins(plugins, event.event_name()) {
        let executable = plugin.executable.clone();
        let name = plugin.name().to_string();
        let event_name = event.event_name().to_string();
        let args = HookArgs {
            endpoint: endpoint.to_string(),
            item: item.clone(),
            category: category.clone(),
        };
        drop(tokio::spawn(async move {
            match invoke_hook(&executable, &event_name, &args).await {
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
        }));
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
        | DaemonEvent::MetadataPatched { item_id } => {
            (Some(item_id.clone()), None)
        }
        DaemonEvent::CategoryChanged { item_id, category } => {
            (item_id.clone(), category.clone())
        }
        DaemonEvent::ScanCompleted { .. } => (None, None),
    }
}

/// One scheduled cron job with its parsed schedule.
struct CronEntry {
    /// Owning plugin name (for logging).
    plugin_name: String,
    /// Plugin executable to spawn.
    executable: PathBuf,
    /// Job id passed back as `cron <id>`.
    job_id: String,
    /// Parsed cron schedule.
    schedule: Schedule,
}

/// Parse every plugin's declared cron jobs, skipping invalid expressions.
fn collect_cron_entries(plugins: &[DiscoveredPlugin]) -> Vec<CronEntry> {
    let mut entries = Vec::new();
    for plugin in plugins {
        for job in &plugin.manifest.cron {
            match Schedule::from_str(&job.schedule) {
                Ok(schedule) => entries.push(CronEntry {
                    plugin_name: plugin.name().to_string(),
                    executable: plugin.executable.clone(),
                    job_id: job.id.clone(),
                    schedule,
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

/// Sleep until the soonest job is due, fire all due jobs, and repeat.
///
/// No catch-up: the next fire time is always computed forward from now, so
/// jobs missed while the process was down are simply not run.
async fn run_cron_scheduler(
    daemon: LocalrefDaemon,
    plugins: Arc<Vec<DiscoveredPlugin>>,
    endpoint: String,
) {
    let mut schedule: Vec<(CronEntry, DateTime<Utc>)> =
        collect_cron_entries(&plugins)
            .into_iter()
            .filter_map(|entry| {
                let next = entry.schedule.after(&Utc::now()).next()?;
                Some((entry, next))
            })
            .collect();
    if schedule.is_empty() {
        return;
    }

    loop {
        let Some(soonest) = schedule.iter().map(|(_, next)| *next).min() else {
            return;
        };
        let wait = (soonest - Utc::now()).to_std().unwrap_or(Duration::ZERO);
        tokio::time::sleep(wait).await;

        let now = Utc::now();
        let paused = daemon.status().paused_modes.contains(&PauseMode::All);
        for (entry, next) in &mut schedule {
            if *next > now {
                continue;
            }
            if paused {
                tracing::debug!(
                    target: "localref::cron",
                    job = %entry.job_id,
                    "cron job skipped; daemon is paused",
                );
            } else {
                fire_cron(entry, &endpoint);
            }
            *next = entry
                .schedule
                .after(&now)
                .next()
                .unwrap_or_else(|| now + TimeDelta::days(3650));
        }
    }
}

/// Spawn one cron plugin invocation, fire-and-forget.
fn fire_cron(entry: &CronEntry, endpoint: &str) {
    let executable = entry.executable.clone();
    let job = entry.job_id.clone();
    let name = entry.plugin_name.clone();
    let endpoint = endpoint.to_string();
    drop(tokio::spawn(async move {
        match invoke_cron(&executable, &job, &endpoint).await {
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
    }));
}

#[cfg(test)]
mod tests {
    use super::{collect_cron_entries, event_targets, matching_plugins};
    use localref_core::DaemonEvent;
    use localref_plugin::{
        CronJob, DiscoveredPlugin, HookBinding, HookEvent, PluginManifest,
    };
    use std::path::PathBuf;

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
                ui: None,
                hooks: hooks
                    .into_iter()
                    .map(|event| HookBinding { event })
                    .collect(),
                cron,
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
    fn collect_cron_entries_keeps_valid_and_skips_invalid() {
        let plugins = vec![
            plugin(
                "good",
                vec![],
                vec![CronJob {
                    id: "nightly".to_string(),
                    schedule: "0 0 3 * * *".to_string(),
                }],
            ),
            plugin(
                "bad",
                vec![],
                vec![CronJob {
                    id: "broken".to_string(),
                    schedule: "not a cron expr".to_string(),
                }],
            ),
        ];
        let entries = collect_cron_entries(&plugins);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plugin_name, "good");
        assert_eq!(entries[0].job_id, "nightly");
    }
}
