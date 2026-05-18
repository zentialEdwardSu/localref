//! Single executable entry point for Localref.
//!
//! The binary owns all runtime modes so installed Localref has one process
//! boundary. Supporting crates provide protocol, REST, tray, and UI libraries,
//! but they do not expose their own installed binaries.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use clap::{Parser, Subcommand};
use csc::{
    ConnectorEvent, ConnectorImportRequest, ConnectorImportSink,
    serve as serve_csc,
};
use localref_config::LocalrefConfig;
use localref_core::LocalrefDaemon;
use storage::StorageDb;
use tray::{TrayAction, TrayCommandResult, TrayController, status_label};
use types::{
    ConnectorAttachment, ConnectorImport, ConnectorItem, ImportOutcome,
};

/// Start Localref in the selected mode.
fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let config =
        LocalrefConfig::load().expect("failed to load Localref configuration");
    match cli.command.unwrap_or(AppCommand::TrayHost) {
        AppCommand::TrayHost => run_tray_host(config),
        AppCommand::Headless => run_runtime(serve_all(config)),
        AppCommand::Rest => run_runtime(serve_rest(config)),
        AppCommand::Csc => run_runtime(serve_csc_only(config)),
        AppCommand::Ui => launch_ui().map_err(std::io::Error::other),
        AppCommand::Tray { action } => {
            run_tray_action(
                &config,
                action.map(Into::into).unwrap_or(TrayAction::RefreshStatus),
            );
            Ok(())
        }
    }
}

/// Localref desktop daemon command line.
#[derive(Debug, Parser)]
#[command(version, about = "Tray-resident Localref desktop daemon")]
struct Cli {
    /// Runtime command. Defaults to the tray-hosted daemon.
    #[command(subcommand)]
    command: Option<AppCommand>,
}

/// Runtime command selected from CLI arguments.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
enum AppCommand {
    /// Start the tray-resident daemon process.
    TrayHost,
    /// Start REST and connector servers without a tray icon.
    #[command(alias = "serve")]
    Headless,
    /// Start only the REST API for manual diagnostics.
    Rest,
    /// Start only the connector-compatible API for manual diagnostics.
    #[command(alias = "csc-dev")]
    Csc,
    /// Launch the iced simple UI.
    Ui,
    /// Execute one tray action through the same binary.
    Tray {
        /// Tray action to execute. Defaults to refreshing status.
        #[command(subcommand)]
        action: Option<TrayCliAction>,
    },
}

/// Tray action selected from CLI arguments.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
enum TrayCliAction {
    /// Open the Simple UI.
    OpenUi,
    /// Request a library scan.
    Scan,
    /// Pause watcher-driven work.
    PauseWatcher,
    /// Pause write operations.
    PauseWrites,
    /// Resume watcher-driven work.
    ResumeWatcher,
    /// Resume write operations.
    ResumeWrites,
    /// Request tray shutdown.
    Quit,
}

impl From<TrayCliAction> for TrayAction {
    fn from(action: TrayCliAction) -> Self {
        match action {
            TrayCliAction::OpenUi => TrayAction::OpenSimpleUi,
            TrayCliAction::Scan => TrayAction::RunScan,
            TrayCliAction::PauseWatcher => TrayAction::PauseWatcher,
            TrayCliAction::PauseWrites => TrayAction::PauseWrites,
            TrayCliAction::ResumeWatcher => TrayAction::ResumeWatcher,
            TrayCliAction::ResumeWrites => TrayAction::ResumeWrites,
            TrayCliAction::Quit => TrayAction::Quit,
        }
    }
}

/// Start the tray-hosted daemon runtime.
fn run_tray_host(config: LocalrefConfig) -> std::io::Result<()> {
    if config.desktop_quiet_start() {
        detach_console_for_quiet_start();
    } else {
        print_config_summary(&config);
    }
    let daemon = open_daemon(&config);
    let _api_thread = start_api_runtime(
        config.clone(),
        daemon,
        !config.desktop_quiet_start(),
    )?;
    run_native_tray_host(&config)
}

/// Start both long-lived HTTP surfaces.
async fn serve_all(config: LocalrefConfig) -> std::io::Result<()> {
    print_config_summary(&config);
    let daemon = open_daemon(&config);
    let rest = serve_rest_with_daemon(config.clone(), daemon.clone());
    let csc = serve_csc_with_daemon(config, daemon);
    tokio::try_join!(rest, csc).map(|_| ())
}

/// Open the daemon once for all in-process API surfaces.
fn open_daemon(config: &LocalrefConfig) -> LocalrefDaemon {
    let storage = StorageDb::open(config.library_root())
        .expect("failed to open Localref query database");
    LocalrefDaemon::new(storage)
}

/// Run an async server mode from the synchronous command entry point.
fn run_runtime(
    future: impl Future<Output = std::io::Result<()>>,
) -> std::io::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(future)
}

/// Start REST and CSC servers on a background Tokio runtime.
fn start_api_runtime(
    config: LocalrefConfig,
    daemon: LocalrefDaemon,
    print_listeners: bool,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new().name("localref-api-runtime".to_string()).spawn(
        move || {
            let rest_config = config.clone();
            let rest_daemon = daemon.clone();
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to start Localref API runtime");
            runtime.block_on(async move {
                let rest = serve_rest_with_daemon_logging(
                    rest_config,
                    rest_daemon,
                    print_listeners,
                );
                let csc = serve_csc_with_daemon_logging(
                    config,
                    daemon,
                    print_listeners,
                );
                if let Err(error) = tokio::try_join!(rest, csc).map(|_| ()) {
                    eprintln!("localref API runtime stopped: {error}");
                }
            });
        },
    )
}

/// Start only the user-facing REST API.
async fn serve_rest(config: LocalrefConfig) -> std::io::Result<()> {
    let storage = StorageDb::open(config.library_root())
        .expect("failed to open Localref query database");
    serve_rest_with_daemon(config, LocalrefDaemon::new(storage)).await
}

/// Start the REST API using an already-open daemon.
async fn serve_rest_with_daemon(
    config: LocalrefConfig,
    daemon: LocalrefDaemon,
) -> std::io::Result<()> {
    serve_rest_with_daemon_logging(config, daemon, true).await
}

/// Start the REST API and optionally print its listener address.
async fn serve_rest_with_daemon_logging(
    config: LocalrefConfig,
    daemon: LocalrefDaemon,
    print_listener: bool,
) -> std::io::Result<()> {
    if print_listener {
        println!("localref REST listening on http://{}", config.rest_addr());
    }
    rest::serve_with_daemon(config.rest_addr(), daemon).await
}

/// Start only the Zotero Connector-compatible API.
async fn serve_csc_only(config: LocalrefConfig) -> std::io::Result<()> {
    let daemon = LocalrefDaemon::for_library(config.library_root())
        .expect("failed to open Localref daemon");
    serve_csc_with_daemon(config, daemon).await
}

/// Start the connector API using an already-open daemon.
async fn serve_csc_with_daemon(
    config: LocalrefConfig,
    daemon: LocalrefDaemon,
) -> std::io::Result<()> {
    serve_csc_with_daemon_logging(config, daemon, true).await
}

/// Start the connector API and optionally print its listener address.
async fn serve_csc_with_daemon_logging(
    config: LocalrefConfig,
    daemon: LocalrefDaemon,
    print_listener: bool,
) -> std::io::Result<()> {
    let sink = Arc::new(LoggingImportSink::new(daemon));
    if print_listener {
        println!("localref CSC listening on http://{}", config.csc_addr());
    }
    serve_csc(config.csc_addr(), sink).await
}

/// Launch the iced desktop UI inside this process.
#[cfg(feature = "desktop")]
fn launch_ui() -> Result<(), String> {
    ui::desktop_app::launch()
}

/// Report unavailable UI support when the binary was built without desktop UI.
#[cfg(not(feature = "desktop"))]
fn launch_ui() -> Result<(), String> {
    println!("Localref: desktop UI feature is not enabled");
    Ok(())
}

/// Run the native tray loop for the daemon host.
#[cfg(feature = "native-tray")]
fn run_native_tray_host(config: &LocalrefConfig) -> std::io::Result<()> {
    let controller = TrayController::from_config(config);
    let options = if config.desktop_start_hidden() {
        ui::desktop_app::DesktopLaunchOptions::hidden()
    } else {
        ui::desktop_app::DesktopLaunchOptions::visible()
    };
    tray::native::run_native_tray_with_options(controller, options)
        .map_err(std::io::Error::other)
}

/// Fail loudly when the binary was built without native tray support.
#[cfg(not(feature = "native-tray"))]
fn run_native_tray_host(_config: &LocalrefConfig) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "native tray feature is not enabled; use `localref headless` for diagnostics",
    ))
}

/// Execute a tray command without spawning another Localref binary.
fn run_tray_action(config: &LocalrefConfig, action: TrayAction) {
    let controller = TrayController::from_config(config);
    match controller.run_action(action) {
        Ok(TrayCommandResult::Status(status)) => {
            println!("{}", status_label(&status));
        }
        Ok(TrayCommandResult::Snapshot(snapshot)) => {
            println!(
                "Localref: items={} categories={} pending={} events={}",
                snapshot.item_count,
                snapshot.category_count,
                snapshot.pending_count,
                snapshot.event_count
            );
        }
        Ok(TrayCommandResult::Message(message)) => println!("{message}"),
        Ok(TrayCommandResult::UiRequested) => {
            if let Err(message) = launch_ui() {
                println!("Localref: error: {message}");
            }
        }
        Ok(TrayCommandResult::Quit) => println!("Localref: quit requested"),
        Err(message) => println!("Localref: error: {message}"),
    }
}

/// Print current config and library paths before serving.
fn print_config_summary(config: &LocalrefConfig) {
    println!("config: {}", config.source_path().display());
    println!("library: {}", config.library_root().display());
    println!("REST: http://{}", config.rest_addr());
    println!("CSC:  http://{}", config.csc_addr());
}

/// Detach the inherited Windows console for configured quiet tray startup.
#[cfg(windows)]
fn detach_console_for_quiet_start() {
    unsafe extern "system" {
        fn FreeConsole() -> i32;
    }
    // SAFETY: FreeConsole only detaches this process from an inherited console.
    // It does not access Rust-managed memory and failing to detach is harmless.
    unsafe {
        FreeConsole();
    }
}

/// Keep non-Windows quiet startup behavior explicit.
#[cfg(not(windows))]
fn detach_console_for_quiet_start() {}

/// Connector sink that logs incoming connector data and forwards it to core.
struct LoggingImportSink {
    daemon: LocalrefDaemon,
    sessions: Mutex<Vec<PendingImport>>,
}

/// Buffered connector save session.
#[derive(Debug)]
struct PendingImport {
    session_id: Option<String>,
    items: Vec<ConnectorItem>,
    attachments: Vec<ConnectorAttachment>,
    outcome: Option<ImportOutcome>,
}

impl LoggingImportSink {
    /// Create a sink from an already-open daemon facade.
    fn new(daemon: LocalrefDaemon) -> Self {
        Self { daemon, sessions: Mutex::new(Vec::new()) }
    }

    /// Try to import every buffered session that has metadata.
    fn try_import_locked(
        &self,
        sessions: &mut [PendingImport],
    ) -> Result<(), String> {
        for session in
            sessions.iter_mut().filter(|session| session.outcome.is_none())
        {
            let Some(item) = session.items.first().cloned() else {
                continue;
            };
            let outcome = self
                .daemon
                .import_connector_item(ConnectorImport {
                    item,
                    attachments: session.attachments.clone(),
                })
                .map_err(|error| error.to_string())?;
            println!("saved Localref item: {}", outcome.item_dir.display());
            for file in &outcome.written_files {
                println!("  wrote: {}", file.display());
            }
            session.outcome = Some(outcome);
        }
        Ok(())
    }
}

impl ConnectorImportSink for LoggingImportSink {
    fn accept_import(
        &self,
        request: ConnectorImportRequest,
    ) -> Result<(), String> {
        println!("connector import: {} item(s)", request.items.len());
        for item in &request.normalized_items {
            println!("  title: {}", item.title);
            if let Some(item_type) = &item.item_type {
                println!("  type: {item_type}");
            }
            if let Some(abstract_note) = &item.abstract_note {
                println!("  abstract: {abstract_note}");
            }
        }
        let mut sessions =
            self.sessions.lock().expect("connector sessions mutex poisoned");
        sessions.push(PendingImport {
            session_id: request.session_id,
            items: request.normalized_items,
            attachments: Vec::new(),
            outcome: None,
        });
        self.try_import_locked(&mut sessions)
    }

    fn accept_attachment(
        &self,
        attachment: ConnectorAttachment,
    ) -> Result<(), String> {
        println!(
            "connector attachment: {} bytes, file {}",
            attachment.bytes.len(),
            attachment.filename
        );
        let mut sessions =
            self.sessions.lock().expect("connector sessions mutex poisoned");
        let session_index = sessions
            .iter()
            .position(|session| session.session_id == attachment.session_id)
            .or_else(|| sessions.len().checked_sub(1));
        let Some(session_index) = session_index else {
            return Err(
                "attachment arrived before any saveItems request".to_string()
            );
        };
        let session = &mut sessions[session_index];
        if let Some(outcome) = &session.outcome {
            let path = self
                .daemon
                .save_connector_attachment_to_item(
                    &outcome.item_dir,
                    attachment,
                )
                .map_err(|error| error.to_string())?;
            println!("  wrote: {}", path.display());
        } else {
            session.attachments.push(attachment);
            self.try_import_locked(&mut sessions)?;
        }
        Ok(())
    }

    fn accept_event(&self, event: ConnectorEvent) -> Result<(), String> {
        println!(
            "connector event: {}",
            serde_json::to_string(&event).map_err(|error| error.to_string())?
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_level_commands_with_clap() {
        assert_eq!(Cli::parse_from(["localref"]).command, None);
        assert_eq!(
            Cli::parse_from(["localref", "serve"]).command,
            Some(AppCommand::Headless)
        );
        assert_eq!(
            Cli::parse_from(["localref", "ui"]).command,
            Some(AppCommand::Ui)
        );
        assert_eq!(
            Cli::parse_from(["localref", "csc-dev"]).command,
            Some(AppCommand::Csc)
        );
    }

    #[test]
    fn parses_tray_subcommands_with_clap() {
        assert_eq!(
            Cli::parse_from(["localref", "tray", "scan"]).command,
            Some(AppCommand::Tray { action: Some(TrayCliAction::Scan) })
        );
    }
}
