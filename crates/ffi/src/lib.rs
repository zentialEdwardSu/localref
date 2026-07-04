//! UniFFI cdylib exposing the Localref core to the Avalonia C# app.
//!
//! The Avalonia process loads this library and calls [`start_daemon`], which
//! boots a Tokio runtime hosting the REST + CSC servers, notification consumer,
//! and plugin workers (mirroring the old tray binary), then returns a
//! [`DaemonHandle`]. The daemon facade methods are synchronous and run directly
//! on the calling C# thread; only server startup and async plugin spawning use
//! the runtime. The REST server keeps serving plugins over `--endpoint`.
//!
//! Types crossing the boundary live in [`dto`]; errors map to [`FfiError`].

mod dto;
mod error;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use localref_core::LocalrefDaemon;
use localref_core::config::LocalrefConfig;
use localref_core::storage::StorageDb;
use localref_core::types::CategoryPath;
use localref_host::plugin_host::{
    RunOutcome, build_action_args, decide_run_outcome,
};
use localref_plugin::DiscoveredPlugin;
use tokio::task::JoinHandle;

pub use dto::{
    CategorySummary, DaemonEvent, DaemonStatus, ItemDocument,
    ItemFilesDocument, LogEntry, Metadata, MetadataDocument, PauseMode,
    PluginUiSpec, ScheduledCall, SearchHit,
};
pub use error::FfiError;

uniffi::setup_scaffolding!();

/// Convenient alias for FFI method results.
type FfiResult<T> = std::result::Result<T, FfiError>;

/// Network + library configuration passed from C# to [`start_daemon`].
#[derive(Debug, uniffi::Record)]
pub struct DaemonConfig {
    /// Absolute path to the library root (`~/.localref/libroot`).
    pub library_root: String,
    /// REST bind address, e.g. `127.0.0.1:8787`.
    pub rest_addr: String,
    /// CSC (Zotero Connector) bind address, e.g. `127.0.0.1:23119`.
    pub csc_addr: String,
    /// Public REST endpoint passed to plugins as `--endpoint`.
    pub rest_endpoint: String,
    /// Directory scanned for plugin bundles.
    pub plugins_dir: String,
}

/// User-editable application settings persisted in Localref's config.toml.
#[derive(Debug, uniffi::Record)]
pub struct AppSettings {
    pub config_path: String,
    pub repo_name: String,
    pub library_root: String,
    pub rest_addr: String,
    pub rest_endpoint: String,
    pub csc_addr: String,
    pub start_hidden: bool,
    pub quiet_start: bool,
}

/// Desktop-only layout preferences persisted in Localref's config.toml.
#[derive(Debug, uniffi::Record)]
pub struct DesktopUiSettings {
    pub author_visible: bool,
    pub venue_visible: bool,
    pub year_visible: bool,
    pub type_visible: bool,
    pub categories_visible: bool,
    pub detail_width: u32,
}

/// A discovered plugin plus its declarative UI, for native form rendering.
#[derive(Debug, uniffi::Record)]
pub struct PluginDescriptor {
    /// Plugin machine-readable name.
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Absolute plugin directory.
    pub dir: String,
    /// Whether the plugin is currently enabled.
    pub enabled: bool,
    /// Declarative UI spec (from `ui.toml`), when present.
    pub ui: Option<PluginUiSpec>,
    /// Lifecycle hook event names this plugin binds.
    pub hooks: Vec<String>,
    /// Cron job ids declared by this plugin.
    pub cron: Vec<String>,
}

/// The result of running a plugin action, returned to the UI.
///
/// When `result` is present and `filename` is set, the UI opens a save dialog
/// (`StorageProvider`) and writes `result` to the chosen file. When only
/// `result` is present, the UI shows it inline.
#[derive(Debug, uniffi::Record)]
pub struct PluginRunResult {
    /// `"ok"` or `"error"`.
    pub status: String,
    /// Text content produced by the action, when any.
    pub result: Option<String>,
    /// Suggested, filesystem-safe save filename, when the action produced one.
    pub filename: Option<String>,
    /// MIME type of `result`, when the plugin declared it.
    pub content_type: Option<String>,
    /// Error message when `status` is `"error"`.
    pub message: Option<String>,
}

/// Foreign (C#) listener invoked on each daemon event.
///
/// Called from a Tokio worker thread, so the C# implementation must marshal to
/// the UI thread before touching view models. The listener must outlive the
/// subscription returned by [`DaemonHandle::subscribe_events`].
#[uniffi::export(callback_interface)]
pub trait DaemonEventListener: Send + Sync {
    /// Called after each completed library mutation.
    fn on_event(&self, event: DaemonEvent);
}

/// The live daemon plus the servers, workers, and runtime it owns.
struct HostRuntime {
    daemon: LocalrefDaemon,
    plugins: localref_host::scheduler::SharedPlugins,
    disabled: Arc<RwLock<std::collections::BTreeSet<String>>>,
    rest_endpoint: String,
    library_root: PathBuf,
    /// Directory scanned for plugins; re-read on `rescan_plugins`.
    plugins_dir: PathBuf,
    /// Owned runtime; kept alive for the process. Dropping it stops the servers.
    runtime: tokio::runtime::Runtime,
    /// Event-subscription tasks, aborted on shutdown / unsubscribe.
    subscriptions: Mutex<Vec<JoinHandle<()>>>,
    /// Logging guard; keeps the file appender worker alive. `None` when a
    /// global subscriber was already installed (e.g. a second boot in a test).
    _log_handle: Option<localref_core::logging::LogHandle>,
}

/// FFI handle to a running Localref daemon.
///
/// Returned by [`start_daemon`]; every UI operation goes through its methods.
/// Call [`DaemonHandle::shutdown`] from the app's exit path.
#[derive(uniffi::Object)]
pub struct DaemonHandle {
    inner: Arc<HostRuntime>,
}

impl std::fmt::Debug for DaemonHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonHandle")
            .field("library_root", &self.inner.library_root)
            .field("rest_endpoint", &self.inner.rest_endpoint)
            .finish_non_exhaustive()
    }
}

/// Resolve the on-disk Localref configuration into a [`DaemonConfig`].
///
/// Runs the same resolution the CLI uses: `LOCALREF_CONFIG` env var, else
/// `~/.localref/config.toml`, materializing documented defaults and writing the
/// file on first run. The Avalonia app calls this and passes the result to
/// [`start_daemon`] so the desktop app and the CLI honour one config source.
///
/// # Errors
///
/// Returns [`FfiError::Internal`] when the config cannot be read, parsed, or
/// created (e.g. no home directory, malformed TOML, or an invalid address).
#[uniffi::export]
pub fn load_config() -> FfiResult<DaemonConfig> {
    let config =
        LocalrefConfig::load().map_err(|msg| FfiError::Internal { msg })?;
    Ok(DaemonConfig {
        library_root: config.library_root().display().to_string(),
        rest_addr: config.rest_addr().to_string(),
        csc_addr: config.csc_addr().to_string(),
        rest_endpoint: config.rest_endpoint().to_string(),
        plugins_dir: config.plugins_dir().display().to_string(),
    })
}

/// Load the settings shown by the desktop settings window.
#[uniffi::export]
pub fn load_app_settings() -> FfiResult<AppSettings> {
    let config =
        LocalrefConfig::load().map_err(|msg| FfiError::Internal { msg })?;
    Ok(AppSettings {
        config_path: config.source_path().display().to_string(),
        repo_name: config.workspace_name().to_string(),
        library_root: config.library_root().display().to_string(),
        rest_addr: config.rest_addr().to_string(),
        rest_endpoint: config.rest_endpoint().to_string(),
        csc_addr: config.csc_addr().to_string(),
        start_hidden: config.desktop_start_hidden(),
        quiet_start: config.desktop_quiet_start(),
    })
}

/// Validate and persist user-editable desktop settings.
#[uniffi::export]
pub fn save_app_settings(settings: AppSettings) -> FfiResult<()> {
    let mut config =
        LocalrefConfig::load().map_err(|msg| FfiError::Internal { msg })?;
    config
        .set_workspace_name(settings.repo_name)
        .map_err(|msg| FfiError::InvalidInput { msg })?;
    let library_root = settings.library_root.trim();
    if library_root.is_empty() {
        return Err(FfiError::InvalidInput {
            msg: "library_root must not be empty".to_string(),
        });
    }
    config.set_library_root(library_root);
    config
        .set_rest_addr(&settings.rest_addr)
        .map_err(|msg| FfiError::InvalidInput { msg })?;
    config
        .set_rest_endpoint(settings.rest_endpoint)
        .map_err(|msg| FfiError::InvalidInput { msg })?;
    config
        .set_csc_addr(&settings.csc_addr)
        .map_err(|msg| FfiError::InvalidInput { msg })?;
    config.set_desktop_start_hidden(settings.start_hidden);
    config.set_desktop_quiet_start(settings.quiet_start);
    config.save().map_err(|msg| FfiError::Internal { msg })?;
    Ok(())
}

/// Load desktop-only library layout preferences.
#[uniffi::export]
pub fn load_desktop_ui_settings() -> FfiResult<DesktopUiSettings> {
    let config =
        LocalrefConfig::load().map_err(|msg| FfiError::Internal { msg })?;
    let columns = config.desktop_visible_columns();
    Ok(DesktopUiSettings {
        author_visible: columns.iter().any(|column| column == "Author"),
        venue_visible: columns.iter().any(|column| column == "Venue"),
        year_visible: columns.iter().any(|column| column == "Year"),
        type_visible: columns.iter().any(|column| column == "Type"),
        categories_visible: columns
            .iter()
            .any(|column| column == "Categories"),
        detail_width: config.desktop_detail_width(),
    })
}

/// Persist desktop-only library layout preferences.
#[uniffi::export]
pub fn save_desktop_ui_settings(settings: DesktopUiSettings) -> FfiResult<()> {
    let mut config =
        LocalrefConfig::load().map_err(|msg| FfiError::Internal { msg })?;
    let columns = [
        ("Author", settings.author_visible),
        ("Venue", settings.venue_visible),
        ("Year", settings.year_visible),
        ("Type", settings.type_visible),
        ("Categories", settings.categories_visible),
    ]
    .into_iter()
    .filter(|(_, visible)| *visible)
    .map(|(column, _)| column.to_string())
    .collect();
    config.set_desktop_visible_columns(columns);
    config.set_desktop_detail_width(settings.detail_width);
    config.save().map_err(|msg| FfiError::Internal { msg })?;
    Ok(())
}

/// Boot the daemon: open storage, discover plugins, start the Tokio runtime with
/// the REST + CSC servers, notification consumer, and plugin workers.
///
/// # Errors
///
/// Returns [`FfiError`] when the library cannot be opened, an address cannot be
/// parsed, or a server cannot bind its port.
#[uniffi::export]
pub fn start_daemon(config: DaemonConfig) -> FfiResult<Arc<DaemonHandle>> {
    let library_root = PathBuf::from(&config.library_root);
    let rest_addr: SocketAddr = config
        .rest_addr
        .parse()
        .map_err(|_| FfiError::InvalidInput { msg: "bad rest_addr".into() })?;
    let csc_addr: SocketAddr = config
        .csc_addr
        .parse()
        .map_err(|_| FfiError::InvalidInput { msg: "bad csc_addr".into() })?;

    // Install the tracing subscriber + global log ring buffer so the UI's
    // logs pane (via `events()`) has something to read. Idempotent: returns
    // `None` if a subscriber is already installed in this process.
    let log_handle = localref_core::logging::init(&library_root, false);

    let storage = StorageDb::open(&library_root)
        .map_err(|e| FfiError::Internal { msg: e.to_string() })?;
    let daemon = LocalrefDaemon::new(storage);
    let plugins_dir = PathBuf::from(&config.plugins_dir);
    let discovered = localref_plugin::discover_plugins(&plugins_dir);
    // Aggregate the indexed `extra` fields every discovered plugin declares, so
    // their values participate in search after the first rebuild.
    daemon
        .set_indexed_extra_fields(indexed_extra_fields(&discovered))
        .map_err(|e| FfiError::Internal { msg: e.to_string() })?;
    let plugins: localref_host::scheduler::SharedPlugins =
        Arc::new(RwLock::new(Arc::new(discovered)));
    let disabled = localref_core::plugin_state::load_disabled(&library_root)
        .unwrap_or_default();
    let disabled = Arc::new(RwLock::new(disabled));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| FfiError::Internal { msg: e.to_string() })?;

    // Bind both server ports up front, on the runtime, so a port-in-use error
    // (e.g. a second Localref instance) surfaces here as an `FfiError` the C#
    // app can show — rather than being lost inside the spawned serve task.
    let (rest_listener, csc_listener) = runtime
        .block_on(async {
            let rest = tokio::net::TcpListener::bind(rest_addr).await?;
            let csc = tokio::net::TcpListener::bind(csc_addr).await?;
            Ok::<_, std::io::Error>((rest, csc))
        })
        .map_err(|e| FfiError::Internal {
            msg: format!("could not bind API server ports: {e}"),
        })?;

    // Start notify consumer, plugin workers, and both servers on the runtime.
    // The listeners are already bound above, so the only errors left here are
    // abnormal server termination, which we log.
    {
        let daemon = daemon.clone();
        let plugins = plugins.clone();
        let disabled = disabled.clone();
        let endpoint = config.rest_endpoint.clone();
        drop(runtime.spawn(async move {
            localref_host::notify::start_notify_consumer();
            localref_host::scheduler::spawn_plugin_workers(
                &daemon, plugins, endpoint, disabled,
            );
            let rest = localref_host::server::serve_rest_on(
                rest_listener,
                daemon.clone(),
            );
            let csc = localref_host::server::serve_csc_on(
                csc_listener,
                daemon,
            );
            if let Err(error) = tokio::try_join!(rest, csc) {
                tracing::error!(
                    target: "localref::ffi",
                    %error,
                    "localref API servers stopped",
                );
            }
        }));
    }

    Ok(Arc::new(DaemonHandle {
        inner: Arc::new(HostRuntime {
            daemon,
            plugins,
            disabled,
            rest_endpoint: config.rest_endpoint,
            library_root,
            plugins_dir,
            runtime,
            subscriptions: Mutex::new(Vec::new()),
            _log_handle: log_handle,
        }),
    }))
}

/// Parse a category path string, rejecting invalid values.
fn category(path: &str) -> FfiResult<CategoryPath> {
    CategoryPath::new(path).ok_or_else(|| FfiError::InvalidInput {
        msg: "invalid category path".into(),
    })
}

/// Aggregate the `"namespace.key"` extra fields every plugin declares indexed.
///
/// Fed to [`LocalrefDaemon::set_indexed_extra_fields`] at boot and on rescan so
/// a newly discovered plugin's indexed fields participate in search.
fn indexed_extra_fields(
    plugins: &[DiscoveredPlugin],
) -> std::collections::BTreeSet<String> {
    plugins
        .iter()
        .flat_map(|plugin| plugin.manifest.extra_fields.iter())
        .filter(|field| field.indexed)
        .map(|field| format!("{}.{}", field.namespace, field.key))
        .collect()
}

#[uniffi::export]
impl DaemonHandle {
    // ---- status & control -------------------------------------------------

    /// Current daemon queue status.
    pub fn status(&self) -> DaemonStatus {
        self.inner.daemon.status().into()
    }

    /// Pause one daemon mode.
    pub fn pause(&self, mode: PauseMode) -> DaemonStatus {
        self.inner.daemon.pause(mode.into()).into()
    }

    /// Resume one daemon mode.
    pub fn resume(&self, mode: PauseMode) -> DaemonStatus {
        self.inner.daemon.resume(mode.into()).into()
    }

    /// Rebuild query storage and normalize the library.
    pub fn scan_all(&self) -> FfiResult<()> {
        let _record = self.inner.daemon.scan_all()?;
        Ok(())
    }

    // ---- items ------------------------------------------------------------

    /// List every indexed item.
    pub fn list_items(&self) -> FfiResult<Vec<ItemDocument>> {
        Ok(self
            .inner
            .daemon
            .list_items()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Fetch one item by id, if it exists.
    pub fn get_item(&self, id: String) -> FfiResult<Option<ItemDocument>> {
        Ok(self.inner.daemon.get_item(&id)?.map(Into::into))
    }

    /// List the files present under one item directory.
    pub fn item_files(
        &self,
        item_id: String,
    ) -> FfiResult<Option<ItemFilesDocument>> {
        Ok(self.inner.daemon.item_files(&item_id)?.map(Into::into))
    }

    /// Fetch one item's full metadata plus its revision hash.
    pub fn get_metadata(
        &self,
        id: String,
    ) -> FfiResult<Option<MetadataDocument>> {
        Ok(self.inner.daemon.get_metadata(&id)?.map(Into::into))
    }

    /// Full-text search over indexed items.
    pub fn search(&self, query: String) -> FfiResult<Vec<SearchHit>> {
        Ok(self
            .inner
            .daemon
            .search(&query)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    // ---- mutations --------------------------------------------------------

    /// Patch an item's metadata under optimistic concurrency.
    ///
    /// On a revision mismatch this returns [`FfiError::Conflict`]; the UI should
    /// reload the metadata and re-prompt.
    pub fn patch_metadata(
        &self,
        item_id: String,
        expected_revision: String,
        metadata: Metadata,
    ) -> FfiResult<ItemDocument> {
        let core_metadata: localref_core::model::Metadata = metadata.into();
        Ok(self
            .inner
            .daemon
            .patch_metadata(&item_id, &expected_revision, &core_metadata)?
            .into())
    }

    /// Set or clear one plugin `extra` value on an item.
    ///
    /// Pass `None` for `value` to remove the key. Returns the reindexed item.
    pub fn set_item_extra(
        &self,
        item_id: String,
        namespace: String,
        key: String,
        value: Option<String>,
    ) -> FfiResult<ItemDocument> {
        Ok(self
            .inner
            .daemon
            .set_item_extra(&item_id, &namespace, &key, value.as_deref())?
            .into())
    }

    /// Delete an indexed item and all its category links; returns whether found.
    pub fn delete_item(&self, item_id: String) -> FfiResult<bool> {
        Ok(self.inner.daemon.delete_item(&item_id)?)
    }

    /// Import one file from disk into a new item (path-based; preferred).
    pub fn import_file(&self, path: String) -> FfiResult<()> {
        let _outcome = self.inner.daemon.import_file(PathBuf::from(path))?;
        Ok(())
    }

    /// Import an existing `All/` directory as a new item.
    pub fn import_all_directory(&self, path: String) -> FfiResult<()> {
        let _outcome =
            self.inner.daemon.import_all_directory(PathBuf::from(path))?;
        Ok(())
    }

    /// Add one file from disk to an existing item (path-based; preferred).
    pub fn add_file_to_item(
        &self,
        item_id: String,
        path: String,
    ) -> FfiResult<()> {
        let _doc = self
            .inner
            .daemon
            .add_file_to_item(&item_id, PathBuf::from(path))?;
        Ok(())
    }

    /// Add an uploaded file (raw bytes) to an existing item.
    ///
    /// Prefer [`DaemonHandle::add_file_to_item`] when a real path exists; bytes
    /// are copied across the FFI boundary in full.
    pub fn add_uploaded_file_to_item(
        &self,
        item_id: String,
        filename: String,
        bytes: Vec<u8>,
    ) -> FfiResult<()> {
        let _doc = self
            .inner
            .daemon
            .add_uploaded_file_to_item(&item_id, &filename, &bytes)?;
        Ok(())
    }

    // ---- categories -------------------------------------------------------

    /// List every category and its linked item ids.
    pub fn list_categories(&self) -> FfiResult<Vec<CategorySummary>> {
        Ok(self
            .inner
            .daemon
            .list_categories()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Create an empty category.
    pub fn create_category(&self, path: String) -> FfiResult<CategorySummary> {
        Ok(self.inner.daemon.create_category(category(&path)?)?.into())
    }

    /// Add items to a category. Pass a single-element vec for one item.
    pub fn add_items_category(
        &self,
        item_ids: Vec<String>,
        path: String,
    ) -> FfiResult<CategorySummary> {
        Ok(self
            .inner
            .daemon
            .add_items_category(&item_ids, category(&path)?)?
            .into())
    }

    /// Remove items from a category. Pass a single-element vec for one item.
    pub fn remove_items_category(
        &self,
        item_ids: Vec<String>,
        path: String,
    ) -> FfiResult<CategorySummary> {
        Ok(self
            .inner
            .daemon
            .remove_items_category(&item_ids, category(&path)?)?
            .into())
    }

    /// Rename a category.
    pub fn rename_category(
        &self,
        from: String,
        to: String,
    ) -> FfiResult<CategorySummary> {
        Ok(self
            .inner
            .daemon
            .rename_category(category(&from)?, category(&to)?)?
            .into())
    }

    /// Merge one category into another.
    pub fn merge_category(
        &self,
        from: String,
        to: String,
    ) -> FfiResult<CategorySummary> {
        Ok(self
            .inner
            .daemon
            .merge_category(category(&from)?, category(&to)?)?
            .into())
    }

    // ---- OS open ----------------------------------------------------------

    /// Open an item's folder in the OS file manager; returns whether found.
    pub fn open_item_folder(&self, item_id: String) -> FfiResult<bool> {
        Ok(self.inner.daemon.open_item_folder(&item_id)?)
    }

    /// Open one item-relative file with its default app; returns whether found.
    pub fn open_item_file(
        &self,
        item_id: String,
        relative: String,
    ) -> FfiResult<bool> {
        Ok(self
            .inner
            .daemon
            .open_item_file(&item_id, &PathBuf::from(relative))?)
    }

    // ---- rules & schedules ------------------------------------------------

    /// Read the raw `rules.toml` text.
    pub fn read_rules_text(&self) -> FfiResult<String> {
        Ok(self.inner.daemon.read_rules_text()?)
    }

    /// Validate and persist `rules.toml` text.
    pub fn write_rules_text(&self, text: String) -> FfiResult<()> {
        self.inner.daemon.write_rules_text(&text)?;
        Ok(())
    }

    /// List runtime-registered scheduled calls.
    pub fn list_schedules(&self) -> FfiResult<Vec<ScheduledCall>> {
        Ok(self
            .inner
            .daemon
            .list_schedules()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Register a runtime scheduled call.
    pub fn register_schedule(&self, call: ScheduledCall) -> FfiResult<()> {
        self.inner.daemon.register_schedule(call.into())?;
        Ok(())
    }

    /// Remove a runtime scheduled call by id; returns whether it existed.
    pub fn remove_schedule(&self, id: String) -> FfiResult<bool> {
        Ok(self.inner.daemon.remove_schedule(&id)?)
    }

    // ---- logs & events ----------------------------------------------------

    /// Snapshot of the recent log ring buffer.
    pub fn events(&self) -> FfiResult<Vec<LogEntry>> {
        Ok(self.inner.daemon.events()?.into_iter().map(Into::into).collect())
    }

    /// Subscribe to live daemon events; returns a subscription id.
    ///
    /// The `listener` is invoked on a Tokio worker thread — marshal to the UI
    /// thread in C#. Aborted by [`DaemonHandle::shutdown`].
    pub fn subscribe_events(
        &self,
        listener: Box<dyn DaemonEventListener>,
    ) -> u64 {
        let mut rx = self.inner.daemon.subscribe();
        let handle = self.inner.runtime.spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => listener.on_event(event.into()),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(
                        _,
                    )) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        });
        let mut subs = self
            .inner
            .subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        subs.push(handle);
        u64::try_from(subs.len()).unwrap_or(0)
    }

    // ---- plugins ----------------------------------------------------------

    /// List discovered plugins with their descriptors and UI specs.
    pub fn list_plugins(&self) -> Vec<PluginDescriptor> {
        self.describe_plugins(&self.plugins_snapshot())
    }

    /// Rediscover plugins from the plugins directory without restarting.
    ///
    /// Re-scans `plugins_dir`, swaps the shared plugin list, re-aggregates
    /// indexed `extra` fields (rebuilding the query index so a new plugin's
    /// fields become searchable), and signals the cron scheduler to reload so
    /// manifest cron jobs from added plugins register. Returns the fresh
    /// descriptors.
    ///
    /// # Errors
    /// Returns an error when the index rebuild for the new indexed fields fails.
    pub fn rescan_plugins(&self) -> FfiResult<Vec<PluginDescriptor>> {
        let discovered =
            localref_plugin::discover_plugins(&self.inner.plugins_dir);
        // Rebuild the search index for the new indexed-field set. The field set
        // lives behind a shared lock, so this update is seen by every daemon
        // clone (servers, workers, and FFI search).
        self.inner
            .daemon
            .set_indexed_extra_fields(indexed_extra_fields(&discovered))
            .map_err(|e| FfiError::Internal { msg: e.to_string() })?;
        {
            let mut guard = self
                .inner
                .plugins
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = Arc::new(discovered);
        }
        // Reload the cron schedule so added/removed manifest cron jobs apply.
        self.inner.daemon.notify_schedules_changed();
        Ok(self.list_plugins())
    }

    /// Open a discovered plugin's directory in the platform file manager.
    ///
    /// Resolves the directory from the current plugin snapshot (a
    /// host-controlled path), not from a caller-supplied path. Returns `false`
    /// when the plugin's directory no longer exists.
    ///
    /// # Errors
    /// Returns [`FfiError::NotFound`] when no plugin matches `plugin`, or a
    /// platform error when the file manager cannot be launched.
    pub fn open_plugin_folder(&self, plugin: String) -> FfiResult<bool> {
        let plugin = self.find_plugin(&plugin)?;
        Ok(self.inner.daemon.open_plugin_folder(&plugin.dir)?)
    }

    /// Run a plugin action, returning its structured result.
    ///
    /// Builds argv via the shared host glue (targeting `--endpoint` at the
    /// in-process REST server) and spawns the plugin on the runtime.
    pub fn run_plugin_action(
        &self,
        plugin: String,
        action: String,
        form: std::collections::HashMap<String, String>,
    ) -> FfiResult<PluginRunResult> {
        let plugin = self.find_plugin(&plugin)?;
        let form: BTreeMap<String, String> = form.into_iter().collect();
        let args = build_action_args(
            plugin.ui.as_ref(),
            &action,
            &self.inner.rest_endpoint,
            &form,
        );
        let output = self
            .inner
            .runtime
            .block_on(localref_plugin::invoke_action(
                &plugin.executable,
                &action,
                &args,
            ))
            .map_err(FfiError::from)?;
        // Classify via the shared decision logic so behaviour matches the old
        // REST path; the UI performs the save-dialog side effect itself.
        let (result, filename) = match decide_run_outcome(&action, &output) {
            RunOutcome::Save { filename, content } => {
                (Some(content), Some(filename))
            }
            RunOutcome::Done | RunOutcome::Error { .. } => {
                (output.result, None)
            }
        };
        Ok(PluginRunResult {
            status: output.status,
            result,
            filename,
            content_type: output.content_type,
            message: output.message,
        })
    }

    /// Run a plugin preview action, returning just the preview text.
    pub fn preview_plugin_action(
        &self,
        plugin: String,
        action: String,
        form: std::collections::HashMap<String, String>,
    ) -> FfiResult<String> {
        let plugin = self.find_plugin(&plugin)?;
        let form: BTreeMap<String, String> = form.into_iter().collect();
        let args = build_action_args(
            plugin.ui.as_ref(),
            &action,
            &self.inner.rest_endpoint,
            &form,
        );
        let output = self
            .inner
            .runtime
            .block_on(localref_plugin::invoke_action(
                &plugin.executable,
                &action,
                &args,
            ))
            .map_err(FfiError::from)?;
        if output.status == "ok" {
            Ok(output.result.unwrap_or_default())
        } else {
            Err(FfiError::Plugin {
                msg: output.message.unwrap_or_else(|| "preview failed".into()),
            })
        }
    }

    /// Enable or disable a plugin, persisting the change.
    pub fn set_plugin_enabled(
        &self,
        plugin: String,
        enabled: bool,
    ) -> FfiResult<()> {
        if !self.plugins_snapshot().iter().any(|p| p.name() == plugin) {
            return Err(FfiError::NotFound { msg: plugin });
        }
        {
            let mut disabled = self
                .inner
                .disabled
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if enabled {
                let _ = disabled.remove(&plugin);
            } else {
                let _ = disabled.insert(plugin.clone());
            }
            localref_core::plugin_state::save_disabled(
                &self.inner.library_root,
                &disabled,
            )
            .map_err(FfiError::from)?;
        }
        Ok(())
    }

    // ---- lifecycle --------------------------------------------------------

    /// Stop event subscriptions and background tasks.
    ///
    /// The owned runtime is dropped when the last handle reference is released,
    /// which stops the REST/CSC servers. Call from the app's exit path.
    pub fn shutdown(&self) {
        let mut subs = self
            .inner
            .subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for handle in subs.drain(..) {
            handle.abort();
        }
    }
}

impl DaemonHandle {
    /// Cheap snapshot of the current discovered-plugin list.
    ///
    /// A rescan swaps the shared `Arc` under the write lock; holding this
    /// snapshot keeps the plugins alive for the duration of a read even if a
    /// concurrent rescan replaces the list.
    fn plugins_snapshot(&self) -> Arc<Vec<DiscoveredPlugin>> {
        let guard = self
            .inner
            .plugins
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(&guard)
    }

    /// Build UI descriptors for a plugin snapshot, marking disabled entries.
    fn describe_plugins(
        &self,
        plugins: &[DiscoveredPlugin],
    ) -> Vec<PluginDescriptor> {
        let disabled = self
            .inner
            .disabled
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        plugins
            .iter()
            .map(|plugin| PluginDescriptor {
                name: plugin.name().to_string(),
                description: plugin.manifest.description.clone(),
                dir: plugin.dir.display().to_string(),
                enabled: !disabled.contains(plugin.name()),
                ui: plugin.ui.clone().map(Into::into),
                hooks: plugin
                    .manifest
                    .hooks
                    .iter()
                    .map(|h| h.event.as_str().to_string())
                    .collect(),
                cron: plugin
                    .manifest
                    .cron
                    .iter()
                    .map(|c| c.id.clone())
                    .collect(),
            })
            .collect()
    }

    /// Find a discovered plugin by name, returning an owned clone.
    ///
    /// Returns an owned [`DiscoveredPlugin`] rather than a borrow so the read
    /// lock is released immediately and a concurrent rescan can proceed.
    fn find_plugin(&self, name: &str) -> FfiResult<DiscoveredPlugin> {
        self.plugins_snapshot()
            .iter()
            .find(|p| p.name() == name)
            .cloned()
            .ok_or_else(|| FfiError::NotFound { msg: name.to_string() })
    }
}
