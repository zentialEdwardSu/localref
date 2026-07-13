//! Argv parsing and the action-handler entry point for Rust plugins.

use std::collections::HashMap;

/// A parsed plugin invocation.
#[derive(Debug)]
pub enum Invocation {
    /// `run <action> --endpoint … [--selected …] [--active …] [--param k=v]`.
    Run {
        /// Action id.
        action: String,
        /// Daemon REST base URL.
        endpoint: String,
        /// Selected item ids.
        selected: Vec<String>,
        /// Active item id.
        active: Option<String>,
        /// Form parameters.
        params: HashMap<String, String>,
    },
    /// `hook <event> --endpoint … [--item …] [--category …]`.
    Hook {
        /// Lifecycle event name (e.g. `item_imported`).
        event: String,
        /// Daemon REST base URL.
        endpoint: String,
        /// Affected item id, when the event names one.
        item: Option<String>,
        /// Affected category path, when the event names one.
        category: Option<String>,
    },
    /// `cron <job> --endpoint …`.
    Cron {
        /// Scheduled job id declared in the manifest.
        job: String,
        /// Daemon REST base URL.
        endpoint: String,
    },
    /// `manifest` — author self-check; prints identity and exits.
    Manifest,
}

/// Parse an argv stream (excluding the executable) into an `Invocation`.
#[must_use]
pub fn parse_args(
    mut args: impl Iterator<Item = String>,
) -> Option<Invocation> {
    match args.next()?.as_str() {
        "manifest" => Some(Invocation::Manifest),
        "hook" => {
            let event = args.next()?;
            let mut endpoint = String::new();
            let mut item = None;
            let mut category = None;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--endpoint" => endpoint = args.next().unwrap_or_default(),
                    "--item" => item = args.next(),
                    "--category" => category = args.next(),
                    _ => {}
                }
            }
            if endpoint.is_empty() {
                return None;
            }
            Some(Invocation::Hook { event, endpoint, item, category })
        }
        "cron" => {
            let job = args.next()?;
            let mut endpoint = String::new();
            while let Some(flag) = args.next() {
                if flag == "--endpoint" {
                    endpoint = args.next().unwrap_or_default();
                }
            }
            if endpoint.is_empty() {
                return None;
            }
            Some(Invocation::Cron { job, endpoint })
        }
        "run" => {
            let action = args.next()?;
            let mut endpoint = String::new();
            let mut selected = Vec::new();
            let mut active = None;
            let mut params = HashMap::new();
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--endpoint" => endpoint = args.next().unwrap_or_default(),
                    "--selected" => {
                        selected = args
                            .next()
                            .unwrap_or_default()
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(ToOwned::to_owned)
                            .collect();
                    }
                    "--active" => active = args.next(),
                    "--param" => {
                        if let Some(kv) = args.next()
                            && let Some((k, v)) = kv.split_once('=')
                        {
                            params.insert(k.to_string(), v.to_string());
                        }
                    }
                    _ => {}
                }
            }
            if endpoint.is_empty() {
                return None;
            }
            Some(Invocation::Run {
                action,
                endpoint,
                selected,
                active,
                params,
            })
        }
        _ => None,
    }
}

/// Form parameters passed to an action handler.
pub type Params = HashMap<String, String>;

/// Context handed to an action handler: ids + a ready REST client.
pub struct ActionContext {
    /// Selected item ids (empty when not targeted).
    pub selected: Vec<String>,
    /// Active item id (when targeted).
    pub active: Option<String>,
    /// Parsed form parameters.
    pub params: Params,
    /// REST client bound to the daemon endpoint.
    pub client: localref_client::LocalrefClient,
}

/// Print one `RunOutput` envelope to stdout.
pub fn emit(output: &localref_plugin::RunOutput) {
    println!(
        "{}",
        serde_json::to_string(output).unwrap_or_else(|_| {
            "{\"status\":\"error\",\"message\":\"serialize failed\"}"
                .to_string()
        })
    );
}

#[cfg(test)]
mod tests {
    use super::{Invocation, parse_args};

    #[test]
    fn parses_run_action_with_ids_and_params() {
        let argv = [
            "run",
            "export_bibtex",
            "--endpoint",
            "http://127.0.0.1:8787",
            "--selected",
            "a,b",
            "--param",
            "format=bibtex",
            "--param",
            "note=a = b",
        ]
        .map(str::to_string);
        let Invocation::Run { action, endpoint, selected, active, params } =
            parse_args(argv.into_iter()).expect("parse run")
        else {
            panic!("expected run invocation");
        };
        assert_eq!(action, "export_bibtex");
        assert_eq!(endpoint, "http://127.0.0.1:8787");
        assert_eq!(selected, vec!["a".to_string(), "b".to_string()]);
        assert!(active.is_none());
        assert_eq!(params.get("format").map(String::as_str), Some("bibtex"));
        // A '=' inside the value is preserved (split on first '=' only).
        assert_eq!(params.get("note").map(String::as_str), Some("a = b"));
    }

    #[test]
    fn parses_hook_with_item_and_category() {
        let argv = [
            "hook",
            "item_imported",
            "--endpoint",
            "http://127.0.0.1:24817",
            "--item",
            "lr:zotero:abc",
            "--category",
            "Wireless/RIS",
        ]
        .map(str::to_string);
        let Invocation::Hook { event, endpoint, item, category } =
            parse_args(argv.into_iter()).expect("parse hook")
        else {
            panic!("expected hook invocation");
        };
        assert_eq!(event, "item_imported");
        assert_eq!(endpoint, "http://127.0.0.1:24817");
        assert_eq!(item.as_deref(), Some("lr:zotero:abc"));
        assert_eq!(category.as_deref(), Some("Wireless/RIS"));
    }

    #[test]
    fn parses_cron_job() {
        let argv =
            ["cron", "nightly_sync", "--endpoint", "http://127.0.0.1:24817"]
                .map(str::to_string);
        let Invocation::Cron { job, endpoint } =
            parse_args(argv.into_iter()).expect("parse cron")
        else {
            panic!("expected cron invocation");
        };
        assert_eq!(job, "nightly_sync");
        assert_eq!(endpoint, "http://127.0.0.1:24817");
    }

    #[test]
    fn hook_without_endpoint_returns_none() {
        let argv = ["hook".to_string(), "item_imported".to_string()];
        assert!(parse_args(argv.into_iter()).is_none());
    }

    #[test]
    fn parses_manifest_subcommand() {
        let argv = ["manifest"].map(str::to_string);
        assert!(matches!(
            parse_args(argv.into_iter()),
            Some(Invocation::Manifest)
        ));
    }

    #[test]
    fn parse_args_empty_returns_none() {
        let argv: [String; 0] = [];
        assert!(parse_args(argv.into_iter()).is_none());
    }

    #[test]
    fn parse_args_unknown_subcommand_returns_none() {
        let argv = ["frobnicate".to_string()];
        assert!(parse_args(argv.into_iter()).is_none());
    }

    #[test]
    fn parse_args_run_without_action_returns_none() {
        let argv = ["run".to_string()];
        assert!(parse_args(argv.into_iter()).is_none());
    }
}
