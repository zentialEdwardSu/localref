//! Error types for the Localref plugin system.

/// Errors that can occur during plugin discovery and invocation.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// Plugin subprocess failed to start or communicate.
    #[error("plugin subprocess error: {0}")]
    Subprocess(String),

    /// Plugin execution timed out.
    #[error("plugin timed out after 30 seconds")]
    Timeout,

    /// Plugin returned an error status.
    #[error("plugin error: {0}")]
    Plugin(String),

    /// Failed to parse plugin output or manifest.
    #[error("parse error: {0}")]
    Parse(String),

    /// Plugin not found.
    #[error("plugin not found: {0}")]
    NotFound(String),

    /// I/O error from the filesystem.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
