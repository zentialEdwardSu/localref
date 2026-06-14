//! Single executable entry point for Localref.
//!
//! The binary owns all runtime modes so installed Localref has one process
//! boundary. Supporting crates provide protocol, REST, tray, and UI libraries,
//! but they do not expose their own installed binaries.

use std::sync::Arc;
use std::thread::JoinHandle;

mod tray;
mod ui;

use clap::{Parser, Subcommand};
use csc::serve as serve_csc;
use localref_core::LocalrefDaemon;
use localref_core::config::LocalrefConfig;
use localref_core::storage::StorageDb;
use tray::{TrayAction, TrayCommandResult, TrayController, status_label};

/// Start Localref in the selected mode.
fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let config =
        LocalrefConfig::load().expect("failed to load Localref configuration");
    let _log_handle = localref_core::logging::init(
        config.library_root(),
        config.desktop_quiet_start(),
    );
    match cli.command.unwrap_or(AppCommand::TrayHost) {
        AppCommand::TrayHost => {
            let runtime = AppRuntime::bootstrap(config)?;
            run_tray_host(runtime)
        }
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
    /// Open the browser-served UI.
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
    /// Open the web UI.
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
fn run_tray_host(runtime: AppRuntime) -> std::io::Result<()> {
    tracing::info!(target: "localref::tray_host", "tray host starting");
    let config = runtime.config.clone();
    if config.desktop_quiet_start() {
        detach_console_for_quiet_start();
    } else {
        print_config_summary(&config);
    }
    let _api_thread = start_api_runtime(runtime)?;
    run_native_tray_host(&config)
}

/// Process-wide runtime built once and shared by every mode.
///
/// Opens the query database, builds the daemon facade, and discovers plugins a
/// single time so every runtime mode starts from the same prepared state.
struct AppRuntime {
    /// Loaded configuration owned for the process lifetime.
    config: LocalrefConfig,
    /// Daemon facade backed by the query database.
    daemon: LocalrefDaemon,
    /// Plugins discovered once at startup.
    plugins: Arc<Vec<localref_plugin::DiscoveredPlugin>>,
}

impl AppRuntime {
    /// Open the daemon and discover plugins once.
    ///
    /// # Errors
    ///
    /// Returns an error when the query database cannot be opened.
    fn bootstrap(config: LocalrefConfig) -> std::io::Result<Self> {
        let storage = StorageDb::open(config.library_root())
            .map_err(std::io::Error::other)?;
        let daemon = LocalrefDaemon::new(storage);
        let plugins =
            Arc::new(localref_plugin::discover_plugins(config.plugins_dir()));
        Ok(Self { config, daemon, plugins })
    }
}

/// Start REST and CSC servers on a background Tokio runtime.
fn start_api_runtime(
    runtime: AppRuntime,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("localref-api-runtime".to_string())
        .spawn(move || {
            let tokio_rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to start Localref API runtime");
            tokio_rt.block_on(async move {
                let rest = serve_rest_with_daemon(
                    runtime.config.clone(),
                    runtime.daemon.clone(),
                    runtime.plugins.clone(),
                );
                let csc = serve_csc_with_daemon(
                    runtime.config.clone(),
                    runtime.daemon.clone(),
                );
                if let Err(error) = tokio::try_join!(rest, csc).map(|_| ()) {
                    tracing::error!(
                        target: "localref::runtime",
                        "localref API runtime stopped: {error}",
                    );
                    eprintln!("localref API runtime stopped: {error}");
                }
            });
        })
}

/// Start the REST API using an already-open daemon.
async fn serve_rest_with_daemon(
    config: LocalrefConfig,
    daemon: LocalrefDaemon,
    plugins: Arc<Vec<localref_plugin::DiscoveredPlugin>>,
) -> std::io::Result<()> {
    println!("localref REST listening on http://{}", config.rest_addr());
    tracing::info!(
        target: "localref::rest",
        "listening on http://{}",
        config.rest_addr(),
    );
    let listener = tokio::net::TcpListener::bind(config.rest_addr()).await?;
    axum::serve(listener, rest_app(&config, daemon, plugins)).await
}

/// Build the REST listener application.
#[cfg(feature = "desktop")]
fn rest_app(
    config: &LocalrefConfig,
    daemon: LocalrefDaemon,
    plugins: Arc<Vec<localref_plugin::DiscoveredPlugin>>,
) -> axum::Router {
    let plugin_context = ui_app::PluginHostContext {
        rest_endpoint: config.rest_endpoint().to_string(),
    };
    localref_core::rest::router_with_daemon(daemon.clone()).merge(
        ui_app::router_with_daemon_repo_plugins_and_context(
            daemon,
            config.repo_name().to_string(),
            plugins,
            plugin_context,
        ),
    )
}

/// Build the REST listener application.
#[cfg(not(feature = "desktop"))]
fn rest_app(
    _config: &LocalrefConfig,
    daemon: LocalrefDaemon,
    _plugins: Arc<Vec<localref_plugin::DiscoveredPlugin>>,
) -> axum::Router {
    localref_core::rest::router_with_daemon(daemon)
}

/// Start the connector API using an already-open daemon.
async fn serve_csc_with_daemon(
    config: LocalrefConfig,
    daemon: LocalrefDaemon,
) -> std::io::Result<()> {
    let sink = Arc::new(csc::DaemonConnectorSink::new(daemon));
    println!("localref CSC listening on http://{}", config.csc_addr());
    tracing::info!(
        target: "localref::csc",
        "listening on http://{}",
        config.csc_addr(),
    );
    serve_csc(config.csc_addr(), sink).await
}

/// Open the browser-served UI endpoint.
#[cfg(feature = "desktop")]
fn launch_ui() -> Result<(), String> {
    let config = LocalrefConfig::load().map_err(|error| error.to_string())?;
    let endpoint = config.rest_endpoint();
    println!("Localref UI: {endpoint}");
    native_win32::open_uri(endpoint).map_err(|error| error.to_string())
}

/// Report unavailable UI support when the binary was built without web UI.
#[cfg(not(feature = "desktop"))]
fn launch_ui() -> Result<(), String> {
    println!("Localref: web UI feature is not enabled");
    Ok(())
}

/// Run the native tray loop for the daemon host.
#[cfg(feature = "native-tray")]
fn run_native_tray_host(config: &LocalrefConfig) -> std::io::Result<()> {
    let controller = TrayController::from_config(config);
    if !config.desktop_start_hidden() {
        let _ = native_win32::open_uri(config.rest_endpoint());
    }
    tray::native::run_native_tray(controller).map_err(std::io::Error::other)
}

/// Fail loudly when the binary was built without native tray support.
#[cfg(not(feature = "native-tray"))]
fn run_native_tray_host(_config: &LocalrefConfig) -> std::io::Result<()> {
    tracing::error!(target: "localref::tray", "native tray feature is not enabled");
    Err(std::io::Error::other(
        "native tray feature is not enabled; use the localref-rest-dev binary for diagnostics",
    ))
}

/// Execute a tray command without spawning another Localref binary.
fn run_tray_action(config: &LocalrefConfig, action: TrayAction) {
    tracing::info!(
        target: "localref::tray",
        "running tray action: {action:?}",
    );
    let controller = TrayController::from_config(config);
    match controller.run_action(action) {
        Ok(TrayCommandResult::Status(status)) => {
            println!("{}", status_label(&status));
        }
        Ok(TrayCommandResult::Snapshot(snapshot)) => {
            println!(
                "Localref: items={} categories={} logs={}",
                snapshot.item_count,
                snapshot.category_count,
                snapshot.log_count
            );
        }
        Ok(TrayCommandResult::UiRequested) => {
            if let Err(message) = launch_ui() {
                println!("Localref: error: {message}");
            }
        }
        Ok(TrayCommandResult::Quit) => println!("Localref: quit requested"),
        Err(message) => {
            tracing::error!(
                target: "localref::tray",
                "tray action failed: {message}",
            );
            println!("Localref: error: {message}");
        }
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
///
/// In debug builds the console is never detached so that log output remains
/// visible. In release builds the config value controls detachment.
#[cfg(all(windows, not(debug_assertions)))]
fn detach_console_for_quiet_start() {
    let _ = native_win32::detach_console();
}

/// Debug or non-Windows builds never detach the console.
#[cfg(any(not(windows), debug_assertions))]
fn detach_console_for_quiet_start() {}
