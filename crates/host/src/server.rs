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

use crate::notify::notify_router;

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
