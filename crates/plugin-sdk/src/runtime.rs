//! CLI protocol runtime: stdin JSON → dispatch → stdout JSON.

use std::io::Read;

use serde::Deserialize;

use crate::plugin::Plugin;

/// Input envelope received on stdin.
#[derive(Debug, Deserialize)]
struct PluginInput {
    /// Protocol operation to execute.
    mode: String,
    /// Page identifier used by render requests.
    #[serde(default)]
    page: String,
    /// Action identifier used by run requests.
    #[serde(default)]
    action: String,
    /// Parameters supplied to an action.
    #[serde(default)]
    params: std::collections::HashMap<String, String>,
    /// Serialized host UI state.
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
    let parse_state = || {
        parsed
            .state
            .as_ref()
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    };

    match parsed.mode.as_str() {
        "manifest" => {
            let manifest = serde_json::json!({
                "name": plugin.name(),
                "description": plugin.description(),
                "actions": plugin.actions().into_iter().map(|a| {
                    let mount = match a.mount {
                        crate::plugin::ActionMount::ActionButton => "action_button",
                        crate::plugin::ActionMount::ContextMenu => "context_menu",
                    };
                    serde_json::json!({
                        "id": a.id,
                        "label": a.label,
                        "mount": mount,
                    })
                }).collect::<Vec<_>>(),
                "pages": plugin.pages().into_iter().map(|p| {
                    let mount = match p.mount {
                        crate::plugin::PageMount::DetailTab => "detail_tab",
                        crate::plugin::PageMount::MetadataPage => "metadata_page",
                        crate::plugin::PageMount::SelectionPage => "selection_page",
                    };
                    serde_json::json!({
                        "id": p.id,
                        "label": p.label,
                        "mount": mount,
                        "route": p.route,
                    })
                }).collect::<Vec<_>>(),
            });
            println!(
                "{}",
                serde_json::to_string(&manifest).unwrap_or_default()
            );
        }
        "render" => {
            let Some(state) = parse_state() else {
                print_error("invalid state in render input");
                return;
            };

            match plugin.render(&parsed.page, &state) {
                Ok(output) => println!(
                    "{}",
                    serde_json::to_string(&output).unwrap_or_default()
                ),
                Err(error) => {
                    let output =
                        localref_plugin::state::RenderOutput::error(error);
                    println!(
                        "{}",
                        serde_json::to_string(&output).unwrap_or_default()
                    );
                }
            }
        }
        "run" => {
            let Some(state) = parse_state() else {
                print_error("invalid state in run input");
                return;
            };

            match plugin.run(&parsed.action, &parsed.params, &state) {
                Ok(output) => println!(
                    "{}",
                    serde_json::to_string(&output).unwrap_or_default()
                ),
                Err(error) => {
                    let output =
                        localref_plugin::state::RunOutput::error(error);
                    println!(
                        "{}",
                        serde_json::to_string(&output).unwrap_or_default()
                    );
                }
            }
        }
        mode => print_error(&format!("unknown mode: {mode}")),
    }
}

/// Print a protocol error response to stdout.
fn print_error(message: &str) {
    let err = serde_json::json!({
        "status": "error",
        "message": message,
    });
    println!("{}", serde_json::to_string(&err).unwrap_or_default());
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
