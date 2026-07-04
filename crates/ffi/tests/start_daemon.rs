//! Integration test: boot the daemon over the FFI entry point and exercise the
//! read path, mirroring the smoke test the Avalonia app performs on launch.

use localref_ffi::{DaemonConfig, start_daemon};

/// Pick a loopback port that is free right now.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn start_daemon_boots_and_serves_reads() {
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
    };

    let handle = start_daemon(config).expect("daemon boots");

    // Read path: a fresh library has no items, and status reports not-running.
    let items = handle.list_items().expect("list items");
    assert!(items.is_empty());
    let status = handle.status();
    assert!(!status.running);
    assert!(handle.list_plugins().is_empty());

    handle.shutdown();
}

#[test]
fn rescan_plugins_discovers_a_plugin_deployed_after_boot() {
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
