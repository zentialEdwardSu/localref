//! Plugin host glue shared by the REST router and the FFI layer.
//!
//! Plugins are separate CLI processes discovered from `plugin.toml` + `ui.toml`
//! (see `localref-plugin`). This module owns the pure, framework-free logic for
//! turning a submitted form into plugin argv ([`build_action_args`]) and for
//! deciding what to do with a plugin's [`RunOutput`] ([`decide_run_outcome`]).
//!
//! Neither function touches Axum or any UI toolkit, so both the in-process REST
//! router (for browser/programmatic clients) and the UniFFI `DaemonHandle` (for
//! the Avalonia app) call the same code — the behaviour cannot diverge.

use std::collections::BTreeMap;

use localref_plugin::manifest::{PluginUiSpec, UiTarget};
use localref_plugin::{ActionArgs, RunOutput};

/// Reserved control keys that never become plugin `--param` entries.
const RESERVED: [&str; 5] =
    ["plugin_action", "action", "return_to", "selected", "active"];

/// Resolve the action's declared target from the UI spec, then collect targeted
/// ids and non-reserved params into the argv inputs for a plugin invocation.
///
/// The declared `target` decides whether the selection csv (`Selection`), the
/// single active id (`Active`), or neither (`None`) is forwarded; every other
/// form field becomes a `--param key=value` entry in stable key order.
#[must_use]
pub fn build_action_args(
    ui: Option<&PluginUiSpec>,
    action_name: &str,
    endpoint: &str,
    form: &BTreeMap<String, String>,
) -> ActionArgs {
    let target = ui.map_or(UiTarget::None, |ui| {
        ui.actions
            .iter()
            .find(|a| a.id == action_name)
            .map(|a| a.target)
            .or_else(|| {
                ui.pages
                    .iter()
                    .find(|p| p.action.as_deref() == Some(action_name))
                    .map(|p| p.target)
            })
            .unwrap_or(UiTarget::None)
    });

    let selected_csv = form.get("selected").cloned().unwrap_or_default();
    let selected: Vec<String> = selected_csv
        .split(',')
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    let active = form.get("active").cloned().filter(|s| !s.is_empty());

    let params: Vec<(String, String)> = form
        .iter()
        .filter(|(k, _)| !RESERVED.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    ActionArgs {
        endpoint: endpoint.to_string(),
        selected: matches!(target, UiTarget::Selection)
            .then_some(selected)
            .unwrap_or_default(),
        active: matches!(target, UiTarget::Active).then_some(active).flatten(),
        params,
    }
}

/// What the host should do with a plugin action's [`RunOutput`].
///
/// This is the framework-free decision the REST handler and the FFI layer share.
/// The caller (Axum redirect, or Avalonia save dialog) performs the side effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// The action succeeded with no downloadable result; nothing to save.
    Done,
    /// The action produced text to save under `filename` (already sanitized).
    Save {
        /// Suggested, filesystem-safe file name for a save dialog.
        filename: String,
        /// The text content to write.
        content: String,
    },
    /// The action failed; `message` describes why.
    Error {
        /// Human-readable failure message.
        message: String,
    },
}

/// Classify a plugin [`RunOutput`] into the host-side [`RunOutcome`].
///
/// A success envelope carrying `result` text becomes [`RunOutcome::Save`] with a
/// safe filename (the plugin's suggested `filename`, else one derived from the
/// action name). A success envelope with no `result` is [`RunOutcome::Done`]. An
/// error envelope becomes [`RunOutcome::Error`].
#[must_use]
pub fn decide_run_outcome(
    action_name: &str,
    output: &RunOutput,
) -> RunOutcome {
    if output.status == "ok" {
        return match output.result.as_deref() {
            Some(result) => RunOutcome::Save {
                filename: output.filename.as_deref().map_or_else(
                    || default_download_filename(action_name),
                    safe_download_filename,
                ),
                content: result.to_string(),
            },
            None => RunOutcome::Done,
        };
    }
    RunOutcome::Error {
        message: output
            .message
            .as_deref()
            .unwrap_or("plugin action failed")
            .to_string(),
    }
}

/// Build the fallback download filename from an action name.
#[must_use]
pub fn default_download_filename(action_name: &str) -> String {
    format!("localref-{}.txt", safe_download_filename(action_name))
}

/// Reduce an arbitrary string to a filesystem-safe download filename.
#[must_use]
pub fn safe_download_filename(value: &str) -> String {
    let safe: String = value
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                Some(ch)
            } else if ch.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect();
    if safe.is_empty() { "localref-export.txt".to_string() } else { safe }
}

#[cfg(test)]
mod tests {
    use super::{RunOutcome, build_action_args, decide_run_outcome};
    use localref_plugin::RunOutput;
    use localref_plugin::manifest::{
        FieldKind, PluginUiSpec, UiAction as SpecAction, UiField, UiMount,
        UiPage, UiTarget,
    };
    use std::collections::BTreeMap;

    /// Build a UI spec with one action and one form page for arg tests.
    fn arg_test_ui() -> PluginUiSpec {
        PluginUiSpec {
            actions: vec![SpecAction {
                id: "export_ris".to_string(),
                label: "Export RIS".to_string(),
                mount: UiMount::ContextMenu,
                target: UiTarget::Selection,
            }],
            pages: vec![UiPage {
                id: "active_form".to_string(),
                label: "Active Form".to_string(),
                mount: UiMount::DetailTab,
                route: "active".to_string(),
                action: Some("export_active".to_string()),
                target: UiTarget::Active,
                requires: Vec::new(),
                preview: None,
                fields: vec![UiField {
                    name: "format".to_string(),
                    label: "Format".to_string(),
                    kind: FieldKind::Text,
                    options: Vec::new(),
                    default: None,
                    required: false,
                    show_if: None,
                    enabled_if: None,
                }],
                display: Vec::new(),
            }],
        }
    }

    fn form(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn build_action_args_selection_target_splits_and_trims_ids() {
        let ui = arg_test_ui();
        let args = build_action_args(
            Some(&ui),
            "export_ris",
            "http://127.0.0.1:7777",
            &form(&[("selected", "a,,b,"), ("format", "bibtex")]),
        );

        assert_eq!(args.endpoint, "http://127.0.0.1:7777");
        assert_eq!(args.selected, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(args.active, None);
        assert_eq!(
            args.params,
            vec![("format".to_string(), "bibtex".to_string())]
        );
    }

    #[test]
    fn build_action_args_active_target_takes_active_and_ignores_selected() {
        let ui = arg_test_ui();
        let args = build_action_args(
            Some(&ui),
            "export_active",
            "",
            &form(&[("active", "lr:zotero:one"), ("selected", "a,b")]),
        );

        assert!(args.selected.is_empty());
        assert_eq!(args.active.as_deref(), Some("lr:zotero:one"));
    }

    #[test]
    fn build_action_args_unknown_action_targets_nothing() {
        let ui = arg_test_ui();
        let args = build_action_args(
            Some(&ui),
            "does_not_exist",
            "",
            &form(&[("active", "lr:zotero:one"), ("selected", "a,b")]),
        );

        assert!(args.selected.is_empty());
        assert_eq!(args.active, None);
    }

    #[test]
    fn build_action_args_no_ui_spec_targets_nothing() {
        let args = build_action_args(
            None,
            "export_ris",
            "",
            &form(&[("selected", "a,b"), ("active", "x")]),
        );

        assert!(args.selected.is_empty());
        assert_eq!(args.active, None);
    }

    #[test]
    fn build_action_args_excludes_reserved_keys_from_params() {
        let ui = arg_test_ui();
        let args = build_action_args(
            Some(&ui),
            "export_ris",
            "",
            &form(&[
                ("plugin_action", "export_ris"),
                ("action", "export_ris"),
                ("return_to", "/?tab=x"),
                ("selected", "a"),
                ("active", "b"),
                ("format", "bibtex"),
            ]),
        );

        assert_eq!(
            args.params,
            vec![("format".to_string(), "bibtex".to_string())]
        );
    }

    #[test]
    fn build_action_args_page_action_fallback_resolves_target() {
        // `export_active` is only a page action (not a top-level UiAction),
        // so the target must come from the page's declared target (Active).
        let ui = arg_test_ui();
        let args = build_action_args(
            Some(&ui),
            "export_active",
            "",
            &form(&[("active", "lr:zotero:one"), ("selected", "a,b")]),
        );

        assert_eq!(args.active.as_deref(), Some("lr:zotero:one"));
        assert!(args.selected.is_empty());
    }

    #[test]
    fn decide_run_outcome_error_status_reports_message() {
        let outcome = decide_run_outcome(
            "export_bibtex",
            &RunOutput::error("no items selected"),
        );
        assert_eq!(
            outcome,
            RunOutcome::Error { message: "no items selected".to_string() }
        );
    }

    #[test]
    fn decide_run_outcome_result_yields_save_with_safe_filename() {
        let outcome = decide_run_outcome(
            "export_bibtex",
            &RunOutput::ok("@article{demo}")
                .content_type("text/x-bibtex")
                .filename("localref-export.bib"),
        );
        assert_eq!(
            outcome,
            RunOutcome::Save {
                filename: "localref-export.bib".to_string(),
                content: "@article{demo}".to_string(),
            }
        );
    }

    #[test]
    fn decide_run_outcome_ok_without_result_is_done() {
        let output = RunOutput {
            status: "ok".to_string(),
            result: None,
            content_type: None,
            filename: None,
            message: None,
        };
        let outcome = decide_run_outcome("noop", &output);
        assert_eq!(outcome, RunOutcome::Done);
    }
}
