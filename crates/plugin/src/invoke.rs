//! Subprocess invocation of plugin CLI binaries.
//!
//! Plugins communicate via stdin JSON and stdout JSON. Each invocation is
//! stateless — the plugin receives the full state it needs and returns a
//! single result.

use std::collections::HashMap;
use std::hash::BuildHasher;

use tokio::process::Command;

use crate::error::PluginError;
use crate::state::{PluginUiState, RenderOutput, RunOutput};

/// Invoke a plugin in `render` mode to produce an SSR HTML fragment.
///
/// # Errors
///
/// Returns an error when the plugin cannot be spawned, times out, exits with a
/// non-zero status, or emits invalid JSON.
pub async fn invoke_render(
    executable: &std::path::Path,
    page: &str,
    state: &PluginUiState,
) -> Result<RenderOutput, PluginError> {
    let input = serde_json::json!({
        "mode": "render",
        "page": page,
        "state": state,
    });
    let output = run_plugin(executable, &input).await?;
    serde_json::from_str(&output)
        .map_err(|error| PluginError::Parse(error.to_string()))
}

/// Invoke a plugin in `run` mode to process an action.
///
/// # Errors
///
/// Returns an error when the plugin cannot be spawned, times out, exits with a
/// non-zero status, or emits invalid JSON.
pub async fn invoke_run<S: BuildHasher + Sync>(
    executable: &std::path::Path,
    action: &str,
    params: &HashMap<String, String, S>,
    state: &PluginUiState,
) -> Result<RunOutput, PluginError> {
    let input = serde_json::json!({
        "mode": "run",
        "action": action,
        "params": params,
        "state": state,
    });
    let output = run_plugin(executable, &input).await?;
    serde_json::from_str(&output)
        .map_err(|error| PluginError::Parse(error.to_string()))
}

/// Spawn a plugin process, write stdin JSON, and collect stdout with a timeout.
async fn run_plugin(
    executable: &std::path::Path,
    input: &serde_json::Value,
) -> Result<String, PluginError> {
    let serialized = serde_json::to_string(input)
        .map_err(|error| PluginError::Parse(error.to_string()))?;

    let mut child = plugin_command(executable).spawn().map_err(|error| {
        PluginError::Subprocess(format!("failed to spawn plugin: {error}"))
    })?;

    // Write stdin using tokio async I/O, then drop to close the pipe.
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            PluginError::Subprocess("failed to open plugin stdin".to_string())
        })?;
        stdin.write_all(serialized.as_bytes()).await.map_err(|error| {
            PluginError::Subprocess(format!("stdin write failed: {error}"))
        })?;
        stdin.shutdown().await.map_err(|error| {
            PluginError::Subprocess(format!("stdin close failed: {error}"))
        })?;
    }

    // Wait with timeout.
    let timeout_duration = std::time::Duration::from_secs(30);
    let output =
        tokio::time::timeout(timeout_duration, child.wait_with_output())
            .await
            .map_err(|_| PluginError::Timeout)?
            .map_err(|error| {
                PluginError::Subprocess(format!("wait failed: {error}"))
            })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PluginError::Plugin(format!(
            "plugin exited with code {:?}: {}",
            output.status.code(),
            stderr.trim()
        )));
    }

    String::from_utf8(output.stdout).map_err(|error| {
        PluginError::Parse(format!("non-UTF-8 output: {error}"))
    })
}

/// Create a plugin subprocess command with hidden-window Windows settings.
fn plugin_command(executable: &std::path::Path) -> Command {
    let mut command = Command::new(executable);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    hide_plugin_window(&mut command);
    command
}

#[cfg(windows)]
fn hide_plugin_window(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_plugin_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invoke_render_detects_missing_executable() {
        let state = PluginUiState {
            repo_name: "Test".to_string(),
            search: None,
            category: None,
            items: Vec::new(),
            categories: Vec::new(),
            selected_ids: Vec::new(),
            active_id: None,
            active_detail: None,
            tab: "metadata".to_string(),
            status_label: "Running".to_string(),
            library_root: "/nonexistent".to_string(),
            rest_endpoint: "http://127.0.0.1:0".to_string(),
        };
        let result = invoke_render(
            std::path::Path::new("/nonexistent/plugin-bin"),
            "main",
            &state,
        )
        .await;
        assert!(result.is_err());
    }
}
