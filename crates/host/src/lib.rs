//! Localref host: process lifecycle, background workers, notifications, and the
//! plugin-host glue shared by the REST router and the FFI layer.
//!
//! This crate owns everything that sat between `localref-core` (the daemon) and
//! the old tray/UI binary: it discovers plugins, builds the merged Axum router
//! (core REST + notify), runs the hook dispatcher and cron scheduler, and drives
//! desktop notifications. The production entry points are the `localref-cli`
//! diagnostics binary and the `localref-ffi` cdylib consumed by the Avalonia app.

pub mod init;
pub mod notify;
pub mod plugin_host;
pub mod scheduler;
pub mod server;

pub use server::{ServerConfig, rest_app, serve_csc_with_daemon, serve_rest};
