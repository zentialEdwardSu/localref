//! CLI protocol runtime: stdin JSON → dispatch → stdout JSON.

use std::io::Read;

use serde::Deserialize;

use crate::plugin::Plugin;

/// Input envelope received on stdin.
#[derive(Debug, Deserialize)]
struct PluginInput {
    mode: String,
    #[serde(default)]
    page: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    params: std::collections::HashMap<String, String>,
    #[serde(default)]
    state: Option<serde_json::Value>,
}

/// Run the plugin CLI protocol loop.
///
/// Reads one JSON object from stdin, dispatches to the plugin's handler,
/// and writes the JSON result to stdout.
pub fn run(plugin: &impl Plugin) {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        print_error("failed to read stdin");
        return;
    }
    let input = input.trim().to_string();
    if input.is_empty() {
        print_error("empty input");
        return;
    }

    let parsed: PluginInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            print_error(&format!("invalid JSON: {e}"));
            return;
        }
    };

    match parsed.mode.as_str() {
        "manifest" => handle_manifest(plugin),
        "render" => handle_render(plugin, &parsed),
        "run" => handle_run(plugin, &parsed),
        mode => print_error(&format!("unknown mode: {mode}")),
    }
}

fn handle_manifest(plugin: &impl Plugin) {
    let manifest = serde_json::json!({
        "name": plugin.name(),
        "description": plugin.description(),
        "actions": plugin.actions().into_iter().map(|a| {
            serde_json::json!({
                "id": a.id,
                "label": a.label,
                "mount": mount_str(&a.mount),
            })
        }).collect::<Vec<_>>(),
        "pages": plugin.pages().into_iter().map(|p| {
            serde_json::json!({
                "id": p.id,
                "label": p.label,
                "mount": page_mount_str(&p.mount),
                "route": p.route,
            })
        }).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string(&manifest).unwrap_or_default());
}

fn handle_render(plugin: &impl Plugin, input: &PluginInput) {
    let Some(state) = input
        .state
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
    else {
        print_error("invalid state in render input");
        return;
    };

    match plugin.render(&input.page, &state) {
        Ok(output) => {
            println!("{}", serde_json::to_string(&output).unwrap_or_default());
        }
        Err(e) => {
            let output = localref_plugin::state::RenderOutput::error(e);
            println!("{}", serde_json::to_string(&output).unwrap_or_default());
        }
    }
}

fn handle_run(plugin: &impl Plugin, input: &PluginInput) {
    let Some(state) = input
        .state
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
    else {
        print_error("invalid state in run input");
        return;
    };

    match plugin.run(&input.action, &input.params, &state) {
        Ok(output) => {
            println!("{}", serde_json::to_string(&output).unwrap_or_default());
        }
        Err(e) => {
            let output = localref_plugin::state::RunOutput::error(e);
            println!("{}", serde_json::to_string(&output).unwrap_or_default());
        }
    }
}

fn print_error(message: &str) {
    let err = serde_json::json!({
        "status": "error",
        "message": message,
    });
    println!("{}", serde_json::to_string(&err).unwrap_or_default());
}

const fn mount_str(mount: &crate::plugin::ActionMount) -> &'static str {
    match mount {
        crate::plugin::ActionMount::ActionButton => "action_button",
        crate::plugin::ActionMount::ContextMenu => "context_menu",
    }
}

const fn page_mount_str(mount: &crate::plugin::PageMount) -> &'static str {
    match mount {
        crate::plugin::PageMount::DetailTab => "detail_tab",
        crate::plugin::PageMount::MetadataPage => "metadata_page",
        crate::plugin::PageMount::SelectionPage => "selection_page",
    }
}

/// Declare the plugin entry point.
///
/// Generates a `main()` function that wires the plugin type into the CLI
/// protocol runtime.  The type must implement `Default + Plugin`.
///
/// # Example
///
/// ```ignore
/// use localref_plugin_sdk::prelude::*;
///
/// #[derive(Default)]
/// struct MyPlugin;
/// impl Plugin for MyPlugin { /* ... */ }
///
/// localref_plugin_main!(MyPlugin);
/// ```
#[macro_export]
macro_rules! localref_plugin_main {
    ($plugin_ty:ty) => {
        fn main() {
            let plugin = <$plugin_ty as Default>::default();
            localref_plugin_sdk::run(&plugin);
        }
    };
}
