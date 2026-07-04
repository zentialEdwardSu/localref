//! Localref command-line entry point.
//!
//! The production desktop app is the Avalonia executable (`app/Localref.Desktop`),
//! which loads the Rust core as a UniFFI cdylib. This CLI covers first-run setup
//! and a headless server mode for development and diagnostics — no tray, no UI.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use localref_core::LocalrefDaemon;
use localref_core::config::LocalrefConfig;
use localref_core::storage::StorageDb;
use localref_host::init;
use localref_host::server::{ServerConfig, serve_csc_with_daemon, serve_rest};

/// Start Localref in the selected mode.
fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    if let Some(AppCommand::Init { repo, force }) = &cli.command {
        return init::run_init(repo.clone(), *force)
            .map_err(std::io::Error::other);
    }
    let config =
        LocalrefConfig::load().expect("failed to load Localref configuration");
    let _log_handle = localref_core::logging::init(
        config.library_root(),
        config.desktop_quiet_start(),
    );
    match cli.command.unwrap_or(AppCommand::Serve) {
        AppCommand::Serve => run_serve(config),
        AppCommand::Init { .. } => unreachable!("init handled above"),
    }
}

/// Localref command line.
#[derive(Debug, Parser)]
#[command(version, about = "Localref core CLI (headless server + setup)")]
struct Cli {
    /// Runtime command. Defaults to the headless server.
    #[command(subcommand)]
    command: Option<AppCommand>,
}

/// Runtime command selected from CLI arguments.
#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
enum AppCommand {
    /// Run the REST + CSC servers headlessly (no tray, no UI).
    Serve,
    /// Initialize configuration and install built-in plugins.
    Init {
        /// Repository (library) path to store in the config.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Overwrite built-in plugins that are already installed.
        #[arg(long)]
        force: bool,
    },
}

/// Run the REST + CSC servers on a Tokio runtime until interrupted.
fn run_serve(config: LocalrefConfig) -> std::io::Result<()> {
    let storage = StorageDb::open(config.library_root())
        .map_err(std::io::Error::other)?;
    let daemon = LocalrefDaemon::new(storage);
    let plugins: localref_host::scheduler::SharedPlugins =
        Arc::new(std::sync::RwLock::new(Arc::new(
            localref_plugin::discover_plugins(config.plugins_dir()),
        )));
    let disabled =
        localref_core::plugin_state::load_disabled(config.library_root())
            .unwrap_or_default();
    let disabled = Arc::new(std::sync::RwLock::new(disabled));

    let server_config = ServerConfig {
        rest_addr: config.rest_addr(),
        csc_addr: config.csc_addr(),
    };

    let runtime =
        tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async move {
        localref_host::notify::start_notify_consumer();
        localref_host::scheduler::spawn_plugin_workers(
            &daemon,
            plugins,
            config.rest_endpoint().to_string(),
            disabled,
        );
        println!("localref REST on http://{}", server_config.rest_addr);
        println!("localref CSC  on http://{}", server_config.csc_addr);
        let rest = serve_rest(server_config, daemon.clone());
        let csc = serve_csc_with_daemon(server_config, daemon);
        tokio::try_join!(rest, csc).map(|_| ())
    })
}
