//! Integration test: boot the daemon over the FFI entry point and exercise the
//! read path, mirroring the smoke test the Avalonia app performs on launch.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use localref_client::{LocalrefClient, NotifyKind};
use localref_ffi::{
    DaemonConfig, DaemonEvent, DaemonEventListener, RuntimeHealthState,
    StatusKind, start_daemon,
};

/// Serializes the tests in this binary. `start_daemon` installs built-in
/// plugins on first run by reading the process-global `LOCALREF_BUILTIN_PLUGINS`
/// env var; one test sets it, so all three must not boot concurrently.
static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Pick a loopback port that is free right now.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn start_daemon_boots_and_serves_reads() {
    let _guard = GUARD.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let plugins_dir = temp.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();

    let rest_port = free_port();
    let csc_port = free_port();
    let config = DaemonConfig {
        library_root: temp.path().display().to_string(),
        rest_addr: format!("127.0.0.1:{rest_port}"),
        csc_addr: format!("127.0.0.1:{csc_port}"),
        rest_endpoint: format!("http://127.0.0.1:{rest_port}"),
        plugins_dir: plugins_dir.display().to_string(),
        log_max_file_bytes: 10 * 1024 * 1024,
        log_backup_count: 2,
    };

    let handle = start_daemon(config).expect("daemon boots");

    // Read path: a fresh library has no items, and status reports not-running.
    let items = handle.list_items().expect("list items");
    assert!(items.is_empty());
    let status = handle.status();
    assert!(!status.running);
    assert!(handle.list_plugins().is_empty());
    let health = handle.runtime_health();
    assert!(matches!(health.state, RuntimeHealthState::Healthy));
    assert_eq!(health.occurrence_count, 0);

    handle.shutdown();
    drop(handle);
    let _rest_listener = std::net::TcpListener::bind(("127.0.0.1", rest_port))
        .expect("REST port is released when the handle is dropped");
    let _csc_listener = std::net::TcpListener::bind(("127.0.0.1", csc_port))
        .expect("CSC port is released when the handle is dropped");
}

#[test]
fn rescan_plugins_discovers_a_plugin_deployed_after_boot() {
    let _guard = GUARD.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let plugins_dir = temp.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();

    let rest_port = free_port();
    let csc_port = free_port();
    let config = DaemonConfig {
        library_root: temp.path().display().to_string(),
        rest_addr: format!("127.0.0.1:{rest_port}"),
        csc_addr: format!("127.0.0.1:{csc_port}"),
        rest_endpoint: format!("http://127.0.0.1:{rest_port}"),
        plugins_dir: plugins_dir.display().to_string(),
        log_max_file_bytes: 10 * 1024 * 1024,
        log_backup_count: 2,
    };

    let handle = start_daemon(config).expect("daemon boots");
    // Nothing at boot: this is the s3sync scenario — the plugin is not present
    // when the daemon starts its one-shot discovery.
    assert!(handle.list_plugins().is_empty());

    // Deploy a plugin (manifest + a matching executable file) after boot.
    let dir = plugins_dir.join("s3sync");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "name = \"s3sync\"\nexecutable = \"s3sync\"\n\n\
         [[cron]]\nid = \"nightly_sync\"\nschedule = \"0 0 3 * * *\"\n",
    )
    .unwrap();
    let exe = if cfg!(windows) { "s3sync.exe" } else { "s3sync" };
    std::fs::write(dir.join(exe), b"").unwrap();

    // Rescan picks it up without a restart, and its manifest cron is reported.
    let plugins = handle.rescan_plugins().expect("rescan");
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].name, "s3sync");
    assert_eq!(plugins[0].cron, vec!["nightly_sync".to_string()]);
    // The swapped list is visible through the ordinary read path too.
    assert_eq!(handle.list_plugins().len(), 1);

    handle.shutdown();
}

/// Test listener that forwards every pushed event to a channel so the test
/// thread can assert on what the UI callback would have received.
struct ChannelListener {
    tx: mpsc::Sender<DaemonEvent>,
}

impl DaemonEventListener for ChannelListener {
    fn on_event(&self, event: DaemonEvent) {
        let _ = self.tx.send(event);
    }
}

#[test]
fn plugin_status_post_reaches_the_event_listener() {
    let _guard = GUARD.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let plugins_dir = temp.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();

    let rest_port = free_port();
    let csc_port = free_port();
    let endpoint = format!("http://127.0.0.1:{rest_port}");
    let config = DaemonConfig {
        library_root: temp.path().display().to_string(),
        rest_addr: format!("127.0.0.1:{rest_port}"),
        csc_addr: format!("127.0.0.1:{csc_port}"),
        rest_endpoint: endpoint.clone(),
        plugins_dir: plugins_dir.display().to_string(),
        log_max_file_bytes: 10 * 1024 * 1024,
        log_backup_count: 2,
    };

    let handle = start_daemon(config).expect("daemon boots");

    // Subscribe exactly as the Avalonia app does, capturing pushed events.
    let (tx, rx) = mpsc::channel();
    let _subscription =
        handle.subscribe_events(Box::new(ChannelListener { tx }));

    // A plugin pushes a status message over REST (the real client path).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let accepted = runtime.block_on(async {
        LocalrefClient::new(endpoint)
            .set_status("syncing 3/10", NotifyKind::Error)
            .await
            .expect("set_status transport")
    });
    assert!(accepted, "endpoint should accept the status message");

    // The push reaches the listener as a StatusMessage carrying text + kind.
    let event = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("listener receives the pushed status");
    match event {
        DaemonEvent::StatusMessage { text, kind } => {
            assert_eq!(text, "syncing 3/10");
            assert!(matches!(kind, StatusKind::Error));
        }
        other => panic!("expected StatusMessage, got {other:?}"),
    }

    handle.shutdown();
}

struct PanicOnceListener {
    calls: AtomicUsize,
    tx: mpsc::Sender<DaemonEvent>,
}

impl DaemonEventListener for PanicOnceListener {
    fn on_event(&self, event: DaemonEvent) {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("injected callback panic");
        }
        let _ = self.tx.send(event);
    }
}

#[test]
fn callback_panic_does_not_end_subscription_loop() {
    let _guard = GUARD.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let plugins_dir = temp.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();
    let rest_port = free_port();
    let csc_port = free_port();
    let endpoint = format!("http://127.0.0.1:{rest_port}");
    let handle = start_daemon(DaemonConfig {
        library_root: temp.path().display().to_string(),
        rest_addr: format!("127.0.0.1:{rest_port}"),
        csc_addr: format!("127.0.0.1:{csc_port}"),
        rest_endpoint: endpoint.clone(),
        plugins_dir: plugins_dir.display().to_string(),
        log_max_file_bytes: 10 * 1024 * 1024,
        log_backup_count: 2,
    })
    .expect("daemon boots");
    let (tx, rx) = mpsc::channel();
    let _subscription = handle.subscribe_events(Box::new(PanicOnceListener {
        calls: AtomicUsize::new(0),
        tx,
    }));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let client = LocalrefClient::new(endpoint);
        assert!(
            client
                .set_status("first", NotifyKind::Info)
                .await
                .expect("first callback")
        );
        assert!(
            client
                .set_status("second", NotifyKind::Info)
                .await
                .expect("second callback")
        );
    });

    let event = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("listener remains subscribed after panic");
    assert!(
        matches!(event, DaemonEvent::StatusMessage { text, .. } if text == "second")
    );
    handle.shutdown();
}

/// Stage one built-in bundle under `dir` (manifest + matching executable).
fn stage_builtin(dir: &std::path::Path, name: &str) {
    let bundle = dir.join(name);
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join("plugin.toml"),
        format!("name = \"{name}\"\nexecutable = \"{name}\"\n"),
    )
    .unwrap();
    let exe =
        if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::fs::write(bundle.join(exe), b"").unwrap();
}

#[test]
fn start_daemon_installs_builtin_plugins_on_first_run_only() {
    let _guard = GUARD.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let plugins_dir = temp.path().join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();

    // Stage a built-in bundle and point the resolver at it.
    let staging = temp.path().join("staging");
    stage_builtin(&staging, "bibtexer");
    // SAFETY: GUARD serializes every test in this binary, so no other test
    // reads the env var concurrently.
    unsafe { std::env::set_var("LOCALREF_BUILTIN_PLUGINS", &staging) };

    let make_config = || {
        let rest_port = free_port();
        let csc_port = free_port();
        DaemonConfig {
            library_root: temp.path().display().to_string(),
            rest_addr: format!("127.0.0.1:{rest_port}"),
            csc_addr: format!("127.0.0.1:{csc_port}"),
            rest_endpoint: format!("http://127.0.0.1:{rest_port}"),
            plugins_dir: plugins_dir.display().to_string(),
            log_max_file_bytes: 10 * 1024 * 1024,
            log_backup_count: 2,
        }
    };

    // First run: empty plugins dir -> the built-in is installed and discovered.
    let handle = start_daemon(make_config()).expect("daemon boots");
    let plugins = handle.list_plugins();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].name, "bibtexer");
    handle.shutdown();
    // Fully drop the handle so its owned runtime stops and the redb database
    // lock is released before the second boot reopens the same library.
    drop(handle);

    // Overwrite the installed manifest so a re-copy would be detectable.
    let installed_manifest = plugins_dir.join("bibtexer").join("plugin.toml");
    std::fs::write(&installed_manifest, "name = \"bibtexer\"\n# edited\n")
        .unwrap();

    // Second run: dir is no longer empty -> install is skipped, edit survives.
    let handle = start_daemon(make_config()).expect("daemon boots");
    assert_eq!(handle.list_plugins().len(), 1);
    handle.shutdown();
    drop(handle);
    let after = std::fs::read_to_string(&installed_manifest).unwrap();
    assert!(after.contains("# edited"), "first-run gate re-copied bundle");

    // SAFETY: GUARD serializes every test in this binary.
    unsafe { std::env::remove_var("LOCALREF_BUILTIN_PLUGINS") };
}
