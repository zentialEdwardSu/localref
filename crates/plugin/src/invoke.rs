//! Argv-driven invocation of plugin CLI binaries.
//!
//! The host spawns the plugin with `run <action> --endpoint … [--selected …]
//! [--active …] [--param k=v …]`, captures one JSON `RunOutput` from stdout,
//! and never pipes a state blob. The same argv runs identically from a shell.

use tokio::process::Command;

use crate::error::PluginError;
use crate::state::{ActionArgs, HookArgs, RunOutput};

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
/// # Errors
/// Returns an error when the plugin cannot be spawned, times out, exits
/// non-zero, or emits invalid JSON.
async fn spawn_and_parse(
    executable: &std::path::Path,
    cmd_args: &[String],
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
        .stderr(std::process::Stdio::piped());
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

    let timeout = std::time::Duration::from_secs(30);
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| PluginError::Timeout)?
        .map_err(|e| PluginError::Subprocess(format!("wait failed: {e}")))?;

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
/// Returns an error when the plugin cannot be spawned, times out, exits
/// non-zero, or emits invalid JSON.
pub async fn invoke_action(
    executable: &std::path::Path,
    action: &str,
    args: &ActionArgs,
) -> Result<RunOutput, PluginError> {
    spawn_and_parse(executable, &build_argv(action, args)).await
}

/// Spawn a plugin hook for a completed daemon event.
///
/// # Errors
/// Returns an error when the plugin cannot be spawned, times out, exits
/// non-zero, or emits invalid JSON.
pub async fn invoke_hook(
    executable: &std::path::Path,
    event: &str,
    args: &HookArgs,
) -> Result<RunOutput, PluginError> {
    spawn_and_parse(executable, &build_hook_argv(event, args)).await
}

/// Spawn a plugin's scheduled cron job.
///
/// # Errors
/// Returns an error when the plugin cannot be spawned, times out, exits
/// non-zero, or emits invalid JSON.
pub async fn invoke_cron(
    executable: &std::path::Path,
    job: &str,
    endpoint: &str,
) -> Result<RunOutput, PluginError> {
    spawn_and_parse(executable, &build_cron_argv(job, endpoint)).await
}

#[cfg(test)]
mod tests {
    use super::{build_argv, build_cron_argv, build_hook_argv};
    use crate::state::{ActionArgs, HookArgs};

    #[test]
    fn hook_argv_includes_event_endpoint_item_and_category() {
        let args = HookArgs {
            endpoint: "http://127.0.0.1:24817".to_string(),
            item: Some("lr:zotero:abc".to_string()),
            category: Some("Wireless/RIS".to_string()),
        };
        let argv = build_hook_argv("item_imported", &args);
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
        let args = HookArgs {
            endpoint: "http://x".to_string(),
            item: None,
            category: None,
        };
        let argv = build_hook_argv("scan_completed", &args);
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
        let args = ActionArgs {
            endpoint: "http://127.0.0.1:8787".to_string(),
            selected: vec!["a".to_string(), "b".to_string()],
            active: None,
            params: vec![("format".to_string(), "bibtex".to_string())],
        };
        let argv = build_argv("export_bibtex", &args);
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
        let args = ActionArgs {
            endpoint: "http://x".to_string(),
            selected: vec![],
            active: Some("id1".to_string()),
            params: vec![("note".to_string(), "a = b\nc d".to_string())],
        };
        let argv = build_argv("act", &args);
        assert!(argv.contains(&"--active".to_string()));
        assert!(argv.contains(&"id1".to_string()));
        assert!(argv.contains(&"note=a = b\nc d".to_string()));
    }

    #[test]
    fn argv_omits_selected_when_empty() {
        let args = ActionArgs {
            endpoint: "http://x".to_string(),
            selected: vec![],
            active: None,
            params: vec![],
        };
        let argv = build_argv("act", &args);
        assert!(!argv.contains(&"--selected".to_string()));
        assert!(!argv.contains(&"--active".to_string()));
    }
}
