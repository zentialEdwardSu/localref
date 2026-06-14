//! Blocking adapter over the async `localref-client` for the tray.
//!
//! The tray event loop is synchronous; this owns a current-thread Tokio
//! runtime and blocks on async client calls. Method shapes mirror the old
//! `RestClient` so `TrayController` is unchanged.

use localref_client::{DaemonStatus, DashboardSnapshot, LocalrefClient};
use localref_core::config::LocalrefConfig;

/// Blocking REST client used by the tray.
#[derive(Clone, Debug)]
pub struct RestClient {
    /// Underlying async client.
    inner: LocalrefClient,
    /// Single-threaded runtime for blocking on async calls.
    runtime: std::sync::Arc<tokio::runtime::Runtime>,
}

impl RestClient {
    /// Build a blocking client for the given REST base URL.
    pub fn new(endpoint: impl Into<String>) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tray runtime");
        Self {
            inner: LocalrefClient::new(endpoint),
            runtime: std::sync::Arc::new(runtime),
        }
    }

    /// Build from a loaded config.
    pub fn from_config(config: &LocalrefConfig) -> Self {
        Self::new(config.rest_endpoint())
    }

    /// Return the configured endpoint.
    pub fn endpoint(&self) -> &str {
        self.inner.endpoint()
    }

    /// Daemon status (blocking).
    pub fn daemon_status(&self) -> Result<DaemonStatus, String> {
        self.runtime
            .block_on(self.inner.daemon_status())
            .map_err(|e| e.to_string())
    }

    /// Dashboard snapshot (blocking).
    pub fn dashboard_snapshot(&self) -> Result<DashboardSnapshot, String> {
        self.runtime
            .block_on(self.inner.dashboard_snapshot())
            .map_err(|e| e.to_string())
    }

    /// Pause a mode (blocking).
    pub fn pause(&self, mode: &str) -> Result<DaemonStatus, String> {
        self.runtime
            .block_on(self.inner.pause(mode))
            .map_err(|e| e.to_string())
    }

    /// Resume a mode (blocking).
    pub fn resume(&self, mode: &str) -> Result<DaemonStatus, String> {
        self.runtime
            .block_on(self.inner.resume(mode))
            .map_err(|e| e.to_string())
    }

    /// Request a scan (blocking).
    pub fn scan(&self) -> Result<serde_json::Value, String> {
        self.runtime
            .block_on(self.inner.scan())
            .map_err(|e| e.to_string())
    }
}
