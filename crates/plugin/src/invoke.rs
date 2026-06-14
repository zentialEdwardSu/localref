//! Argv-driven invocation of plugin CLI binaries.
//!
//! The host spawns the plugin with `run <action> --endpoint … [--selected …]
//! [--active …] [--param k=v …]`, captures one JSON `RunOutput` from stdout,
//! and never pipes a state blob. The same argv runs identically from a shell.

use tokio::process::Command;

use crate::error::PluginError;
use crate::state::{ActionArgs, RunOutput};

/// Build the argv (excluding the executable) for an action invocation.
///
/// Each value is a separate vector entry — the spawn API passes them as
/// distinct OS args, so spaces / `=` / newlines never trigger shell parsing.
// Single caller today (`invoke_action`); kept separate so the argv-construction
// logic is unit-tested directly rather than through a process spawn.
#[allow(clippy::single_call_fn)]
fn build_argv(action: &str, args: &ActionArgs) -> Vec<String> {
    let mut argv = vec!["run".to_string(), action.to_string()];
    argv.push("--endpoint".to_string());
    argv.push(args.endpoint.clone());
    if !args.selected.is_empty() {
        argv.push("--selected".to_string());
        argv.push(args.selected.join(","));
    }
    if let Some(active) = &args.active {
        argv.push("--active".to_string());
        argv.push(active.clone());
    }
    for (key, value) in &args.params {
        argv.push("--param".to_string());
        argv.push(format!("{key}={value}"));
    }
    argv
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
    let cmd_args = build_argv(action, args);

    let mut command = Command::new(executable);
    command
        .args(&cmd_args)
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

#[cfg(test)]
mod tests {
    use super::build_argv;
    use crate::state::ActionArgs;

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
