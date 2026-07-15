//! Argv-driven invocation of plugin CLI binaries.
//!
//! The host spawns the plugin with `run <action> --endpoint … [--selected …]
//! [--active …] [--param k=v …]`, captures one JSON `RunOutput` from stdout,
//! and never pipes a state blob. The same argv runs identically from a shell.
//!
//! Every child is spawned with `kill_on_drop(true)` and, when a
//! [`PluginProcessRegistry`] is supplied, registered for its whole run so the
//! host can list it and cancel it. The run's `select!` resolves on the first of:
//! the process exiting, the timeout elapsing, or a cancel signal — and in the
//! latter two cases the child future is dropped, which kills the OS process.
//! This is what guarantees no plugin child (and thus no inherited listening
//! socket) outlives the app.

use std::sync::Arc;

use tokio::process::Command;

use crate::error::PluginError;
use crate::registry::{InvocationKind, PluginProcessRegistry};
use crate::state::{ActionArgs, HookArgs, RunOutput};

/// Default wall-clock budget for a plugin subprocess when the action does not
/// declare its own. Long-running actions (e.g. a full-library sync) override
/// this via `timeout_secs` in their UI/cron spec.
pub const DEFAULT_PLUGIN_TIMEOUT_SECS: u64 = 30;

/// Lifecycle-tracking context for one invocation.
///
/// Owns an `Arc` clone of the registry (not a borrow) so it can move into the
/// `tokio::spawn` tasks the scheduler uses for fire-and-forget hook/cron runs.
/// When present, the run is registered on spawn and deregistered on exit, and
/// becomes cancellable via [`PluginProcessRegistry::cancel`].
#[derive(Clone)]
pub struct InvocationTracking {
    /// Registry the run registers itself in for its lifetime.
    pub registry: Arc<PluginProcessRegistry>,
    /// Plugin name, shown in the running list.
    pub plugin: String,
    /// Which entry point produced this run.
    pub kind: InvocationKind,
}

/// Build the argv (excluding the executable) for an action invocation.
///
/// Each value is a separate vector entry — the spawn API passes them as
/// distinct OS args, so spaces / `=` / newlines never trigger shell parsing.
// Single caller today (`invoke_action`); kept separate so the argv-construction
// logic is unit-tested directly rather than through a process spawn.
#[allow(clippy::single_call_fn)]
fn build_argv(action: &str, args: &ActionArgs) -> Vec<String> {
    let mut out = vec!["run".to_string(), action.to_string()];
    out.push("--endpoint".to_string());
    out.push(args.endpoint.clone());
    if !args.selected.is_empty() {
        out.push("--selected".to_string());
        // Item ids never contain ',' (format: lr:<connector>:<id>), so a CSV
        // join is unambiguous; the plugin SDK splits this back on ','.
        out.push(args.selected.join(","));
    }
    if let Some(active) = &args.active {
        out.push("--active".to_string());
        out.push(active.clone());
    }
    for (key, value) in &args.params {
        out.push("--param".to_string());
        out.push(format!("{key}={value}"));
    }
    out
}

/// Build the argv (excluding the executable) for a hook invocation.
// Single caller (`invoke_hook`); kept separate so the argv layout is
// unit-tested directly rather than through a process spawn.
#[allow(clippy::single_call_fn)]
fn build_hook_argv(event: &str, args: &HookArgs) -> Vec<String> {
    let mut out = vec!["hook".to_string(), event.to_string()];
    out.push("--endpoint".to_string());
    out.push(args.endpoint.clone());
    if let Some(item) = &args.item {
        out.push("--item".to_string());
        out.push(item.clone());
    }
    if let Some(category) = &args.category {
        out.push("--category".to_string());
        out.push(category.clone());
    }
    out
}

/// Build the argv (excluding the executable) for a cron invocation.
// Single caller (`invoke_cron`); kept separate for direct unit testing.
#[allow(clippy::single_call_fn)]
fn build_cron_argv(job: &str, endpoint: &str) -> Vec<String> {
    vec![
        "cron".to_string(),
        job.to_string(),
        "--endpoint".to_string(),
        endpoint.to_string(),
    ]
}

/// Spawn a plugin with the given argv and parse its single JSON envelope.
///
/// The child is spawned with `kill_on_drop(true)` and, when `tracking` is set,
/// registered for its lifetime so it can be listed and cancelled. The run ends
/// on the first of: process exit, timeout, or cancel; the last two drop the
/// child future, killing the process.
///
/// # Errors
/// Returns an error when the plugin cannot be spawned, times out, is cancelled,
/// exits non-zero, or emits invalid JSON.
async fn spawn_and_parse(
    executable: &std::path::Path,
    cmd_args: &[String],
    timeout_secs: Option<u64>,
    tracking: Option<InvocationTracking>,
    label: &str,
) -> Result<RunOutput, PluginError> {
    tracing::trace!(
        target: "localref::plugins",
        executable = %executable.display(),
        argv = ?cmd_args,
        "spawning plugin process",
    );
    let mut command = Command::new(executable);
    command
        .args(cmd_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Dropping the wait future (on timeout, cancel, or runtime shutdown)
        // must kill the OS process, not detach it — otherwise a hung plugin
        // becomes an orphan that can keep an inherited listening socket bound.
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let child = command.spawn().map_err(|e| {
        PluginError::Subprocess(format!("failed to spawn plugin: {e}"))
    })?;

    // Register for the run's lifetime. The guard deregisters on drop, so every
    // exit path (success, timeout, cancel, error) cleans up. `kill_rx` fires
    // when the registry cancels this id.
    let (_guard, kill_rx) = match tracking {
        Some(tracking) => {
            let (rx, guard) = tracking.registry.register(
                &tracking.plugin,
                label,
                tracking.kind,
            );
            (Some(guard), Some(rx))
        }
        None => (None, None),
    };

    let timeout = std::time::Duration::from_secs(
        timeout_secs.unwrap_or(DEFAULT_PLUGIN_TIMEOUT_SECS),
    );
    let wait = child.wait_with_output();
    tokio::pin!(wait);
    // A never-resolving future stands in for the cancel arm when untracked, so
    // the `select!` shape stays uniform.
    let cancelled = async {
        match kill_rx {
            Some(rx) => {
                let _ = rx.await;
            }
            None => std::future::pending::<()>().await,
        }
    };

    let output = tokio::select! {
        result = &mut wait => result
            .map_err(|e| PluginError::Subprocess(format!("wait failed: {e}")))?,
        () = tokio::time::sleep(timeout) => return Err(PluginError::Timeout),
        () = cancelled => return Err(PluginError::Cancelled),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PluginError::Plugin(format!(
            "plugin exited with code {:?}: {}",
            output.status.code(),
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| PluginError::Parse(format!("non-UTF-8 output: {e}")))?;
    serde_json::from_str(stdout.trim())
        .map_err(|e| PluginError::Parse(e.to_string()))
}

/// Spawn a plugin action and parse its single JSON result envelope.
///
/// # Errors
/// Returns an error when the plugin cannot be spawned, times out, is cancelled,
/// exits non-zero, or emits invalid JSON.
pub async fn invoke_action(
    executable: &std::path::Path,
    action: &str,
    args: &ActionArgs,
    timeout_secs: Option<u64>,
    tracking: Option<InvocationTracking>,
) -> Result<RunOutput, PluginError> {
    spawn_and_parse(
        executable,
        &build_argv(action, args),
        timeout_secs,
        tracking,
        action,
    )
    .await
}

/// Spawn a plugin hook for a completed daemon event.
///
/// # Errors
/// Returns an error when the plugin cannot be spawned, times out, is cancelled,
/// exits non-zero, or emits invalid JSON.
pub async fn invoke_hook(
    executable: &std::path::Path,
    event: &str,
    args: &HookArgs,
    timeout_secs: Option<u64>,
    tracking: Option<InvocationTracking>,
) -> Result<RunOutput, PluginError> {
    spawn_and_parse(
        executable,
        &build_hook_argv(event, args),
        timeout_secs,
        tracking,
        event,
    )
    .await
}

/// Spawn a plugin's scheduled cron job.
///
/// # Errors
/// Returns an error when the plugin cannot be spawned, times out, is cancelled,
/// exits non-zero, or emits invalid JSON.
pub async fn invoke_cron(
    executable: &std::path::Path,
    job: &str,
    endpoint: &str,
    timeout_secs: Option<u64>,
    tracking: Option<InvocationTracking>,
) -> Result<RunOutput, PluginError> {
    spawn_and_parse(
        executable,
        &build_cron_argv(job, endpoint),
        timeout_secs,
        tracking,
        job,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        InvocationTracking, build_argv, build_cron_argv, build_hook_argv,
        spawn_and_parse,
    };
    use crate::error::PluginError;
    use crate::registry::{InvocationKind, PluginProcessRegistry};
    use crate::state::{ActionArgs, HookArgs};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    /// A long-running process usable as a stand-in plugin that never exits on
    /// its own within a test's lifetime, so timeout/cancel paths can be driven.
    fn sleeper() -> (PathBuf, Vec<String>) {
        if cfg!(windows) {
            (
                PathBuf::from("cmd"),
                vec![
                    "/C".into(),
                    "ping".into(),
                    "-n".into(),
                    "30".into(),
                    "127.0.0.1".into(),
                ],
            )
        } else {
            (PathBuf::from("sleep"), vec!["30".into()])
        }
    }

    #[test]
    fn hook_argv_includes_event_endpoint_item_and_category() {
        let hook_args = HookArgs {
            endpoint: "http://127.0.0.1:24817".to_string(),
            item: Some("lr:zotero:abc".to_string()),
            category: Some("Wireless/RIS".to_string()),
        };
        let argv = build_hook_argv("item_imported", &hook_args);
        assert_eq!(argv[0], "hook");
        assert_eq!(argv[1], "item_imported");
        assert!(argv.contains(&"--endpoint".to_string()));
        assert!(argv.contains(&"--item".to_string()));
        assert!(argv.contains(&"lr:zotero:abc".to_string()));
        assert!(argv.contains(&"--category".to_string()));
        assert!(argv.contains(&"Wireless/RIS".to_string()));
    }

    #[test]
    fn hook_argv_omits_absent_item_and_category() {
        let hook_args = HookArgs {
            endpoint: "http://x".to_string(),
            item: None,
            category: None,
        };
        let argv = build_hook_argv("scan_completed", &hook_args);
        assert!(!argv.contains(&"--item".to_string()));
        assert!(!argv.contains(&"--category".to_string()));
    }

    #[test]
    fn cron_argv_carries_job_and_endpoint() {
        let argv = build_cron_argv("nightly_sync", "http://127.0.0.1:24817");
        assert_eq!(argv[0], "cron");
        assert_eq!(argv[1], "nightly_sync");
        assert!(argv.contains(&"--endpoint".to_string()));
        assert!(argv.contains(&"http://127.0.0.1:24817".to_string()));
    }

    #[test]
    fn argv_includes_endpoint_selected_and_params() {
        let action_args = ActionArgs {
            endpoint: "http://127.0.0.1:8787".to_string(),
            selected: vec!["a".to_string(), "b".to_string()],
            active: None,
            params: vec![("format".to_string(), "bibtex".to_string())],
        };
        let argv = build_argv("export_bibtex", &action_args);
        assert_eq!(argv[0], "run");
        assert_eq!(argv[1], "export_bibtex");
        assert!(argv.contains(&"--endpoint".to_string()));
        assert!(argv.contains(&"http://127.0.0.1:8787".to_string()));
        assert!(argv.contains(&"--selected".to_string()));
        assert!(argv.contains(&"a,b".to_string()));
        assert!(argv.contains(&"--param".to_string()));
        assert!(argv.contains(&"format=bibtex".to_string()));
    }

    #[test]
    fn argv_passes_special_chars_as_intact_entries() {
        let action_args = ActionArgs {
            endpoint: "http://x".to_string(),
            selected: vec![],
            active: Some("id1".to_string()),
            params: vec![("note".to_string(), "a = b\nc d".to_string())],
        };
        let argv = build_argv("act", &action_args);
        assert!(argv.contains(&"--active".to_string()));
        assert!(argv.contains(&"id1".to_string()));
        assert!(argv.contains(&"note=a = b\nc d".to_string()));
    }

    #[test]
    fn argv_omits_selected_when_empty() {
        let action_args = ActionArgs {
            endpoint: "http://x".to_string(),
            selected: vec![],
            active: None,
            params: vec![],
        };
        let argv = build_argv("act", &action_args);
        assert!(!argv.contains(&"--selected".to_string()));
        assert!(!argv.contains(&"--active".to_string()));
    }

    /// Cancelling a tracked, still-running invocation aborts it promptly with
    /// `Cancelled` and deregisters it — the mechanism the UI "Cancel" button and
    /// shutdown `cancel_all` both rely on.
    #[tokio::test]
    async fn cancel_via_registry_returns_cancelled_and_deregisters() {
        let registry = Arc::new(PluginProcessRegistry::new());
        let (exe, args) = sleeper();
        let tracking = InvocationTracking {
            registry: Arc::clone(&registry),
            plugin: "sleeper".into(),
            kind: InvocationKind::Action,
        };
        let run = tokio::spawn(async move {
            // Generous timeout so the *cancel* path (not timeout) is exercised.
            spawn_and_parse(&exe, &args, Some(120), Some(tracking), "act")
                .await
        });

        // Wait until the invocation registers, then cancel it by id.
        let id = loop {
            if let Some(first) = registry.list().first() {
                break first.id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(registry.cancel(id));

        let result = run.await.expect("join");
        assert!(matches!(result, Err(PluginError::Cancelled)));
        assert!(registry.list().is_empty(), "run must deregister on cancel");
    }

    /// A run that outlives its declared timeout returns `Timeout` (and the child
    /// is killed by `kill_on_drop` when the wait future is dropped).
    #[tokio::test]
    async fn timeout_returns_timeout_error() {
        let (exe, args) = sleeper();
        let result = spawn_and_parse(&exe, &args, Some(1), None, "act").await;
        assert!(matches!(result, Err(PluginError::Timeout)));
    }
}
