//! Merged HTTP surface and server lifecycle for the Localref host.
//!
//! The core process keeps two in-process HTTP servers alongside the FFI API:
//! the REST server (core data routes plus `POST /api/notify`) that plugins reach
//! over `--endpoint`, and the Zotero Connector (CSC) server. This module builds
//! the merged REST router and the two `serve` futures; the caller supplies the
//! Tokio runtime and drives them (the FFI `start_daemon`, or the CLI binary).

use std::net::SocketAddr;
use std::sync::Arc;

use localref_core::LocalrefDaemon;
use tokio::net::TcpListener;

use crate::notify::notify_router;

/// Clear the inherit flag on a bound listener's socket handle (Windows).
///
/// tokio's listeners come from mio, whose Windows socket creation does not set
/// `WSA_FLAG_NO_HANDLE_INHERIT`, so the handle is inheritable. Plugin children
/// are spawned with piped stdio (`bInheritHandles=TRUE`) and would otherwise
/// inherit the daemon's REST/CSC listening sockets — a lingering child then
/// keeps the port bound after the app exits. Clearing the flag here means a
/// child can never inherit these sockets in the first place.
///
/// No-op on non-Windows platforms, where child processes do not inherit
/// arbitrary fds the way piped stdio implies on Windows.
pub fn deny_socket_inheritance(listener: &TcpListener) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Foundation::{
            HANDLE_FLAG_INHERIT, SetHandleInformation,
        };
        let handle = listener.as_raw_socket() as isize as *mut std::ffi::c_void;
        // SAFETY: `handle` is a live socket handle owned by `listener`, valid
        // for the duration of this call. Clearing the inherit bit is a pure
        // handle-metadata change with no aliasing concerns.
        let ok = unsafe {
            SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0)
        };
        if ok == 0 {
            tracing::warn!(
                target: "localref::server",
                error = %std::io::Error::last_os_error(),
                "could not clear socket inherit flag; a plugin child could pin this port",
            );
        }
    }
    #[cfg(not(windows))]
    {
        let _ = listener;
    }
}

/// Network + identity configuration needed to stand up the servers.
#[derive(Clone, Copy, Debug)]
pub struct ServerConfig {
    /// Address the REST server binds, e.g. `127.0.0.1:8787`.
    pub rest_addr: SocketAddr,
    /// Address the CSC (Zotero Connector) server binds.
    pub csc_addr: SocketAddr,
}

/// Build the merged REST application: core data routes + notification endpoint.
///
/// The old browser UI routes are gone (the UI is now the Avalonia app talking
/// over UniFFI). What remains is exactly what a separate process needs: the
/// `/api/*` data routes plugins consume via `--endpoint`, plus `/api/notify`.
pub fn rest_app(daemon: LocalrefDaemon) -> axum::Router {
    localref_core::rest::router_with_daemon(daemon).merge(notify_router())
}

/// Serve the REST application on `rest_addr` until the process exits.
///
/// # Errors
///
/// Returns an error when the listener cannot bind or the server stops abnormally.
pub async fn serve_rest(
    config: ServerConfig,
    daemon: LocalrefDaemon,
) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.rest_addr).await?;
    serve_rest_on(listener, daemon).await
}

/// Serve the REST application on an already-bound listener.
///
/// Splitting the bind from the serve lets a caller (the FFI `start_daemon`)
/// surface a port-in-use error synchronously instead of losing it inside a
/// spawned task.
///
/// # Errors
///
/// Returns an error when the server stops abnormally.
pub async fn serve_rest_on(
    listener: tokio::net::TcpListener,
    daemon: LocalrefDaemon,
) -> std::io::Result<()> {
    deny_socket_inheritance(&listener);
    tracing::info!(
        target: "localref::rest",
        "listening on http://{}",
        listener.local_addr().map_or_else(
            |_| "127.0.0.1".to_string(),
            |addr| addr.to_string(),
        ),
    );
    axum::serve(listener, rest_app(daemon)).await
}

/// Serve the connector (CSC) API using an already-open daemon.
///
/// # Errors
///
/// Returns an error when the listener cannot bind or the server stops abnormally.
pub async fn serve_csc_with_daemon(
    config: ServerConfig,
    daemon: LocalrefDaemon,
) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.csc_addr).await?;
    serve_csc_on(listener, daemon).await
}

/// Serve the connector (CSC) API on an already-bound listener.
///
/// # Errors
///
/// Returns an error when the server stops abnormally.
pub async fn serve_csc_on(
    listener: tokio::net::TcpListener,
    daemon: LocalrefDaemon,
) -> std::io::Result<()> {
    deny_socket_inheritance(&listener);
    let sink = Arc::new(csc::DaemonConnectorSink::new(daemon));
    tracing::info!(
        target: "localref::csc",
        "listening on http://{}",
        listener.local_addr().map_or_else(
            |_| "127.0.0.1".to_string(),
            |addr| addr.to_string(),
        ),
    );
    csc::serve_on(listener, sink).await
}
