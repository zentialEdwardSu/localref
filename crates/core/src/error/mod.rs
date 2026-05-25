//! Shared Localref error type.
//!
//! Crates use this type when failures should cross crate boundaries. Local
//! helper functions can still use narrower internal errors when appropriate.

use std::path::PathBuf;

/// Result alias for Localref operations.
pub type Result<T> = std::result::Result<T, LocalrefError>;

/// Error variants emitted by Localref crates.
#[derive(Debug, thiserror::Error)]
pub enum LocalrefError {
    /// Filesystem operation failed.
    #[error("filesystem operation failed at {path}: {source}")]
    Io {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Original IO error.
        #[source]
        source: std::io::Error,
    },

    /// JSON serialization or deserialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// TOML serialization or deserialization failed.
    #[error("toml serialization error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    /// TOML deserialization failed.
    #[error("toml deserialization error: {0}")]
    TomlDe(#[from] toml::de::Error),

    /// A required connector payload field is missing.
    #[error("missing required connector field: {0}")]
    MissingField(&'static str),

    /// A filename or path component cannot be written safely.
    #[error("invalid path component `{component}`: {reason}")]
    InvalidPathComponent {
        /// Rejected path component.
        component: String,
        /// Reason the component was rejected.
        reason: &'static str,
    },

    /// A requested operation is not supported by this implementation slice.
    #[error("unsupported operation: {0}")]
    Unsupported(&'static str),

    /// A write conflict prevented a filesystem or metadata operation.
    #[error("conflict: {0}")]
    Conflict(String),

    /// A rules configuration or query is invalid.
    #[error("rule error: {0}")]
    Rule(String),

    /// Storage backend operation failed.
    #[error("storage error: {0}")]
    Storage(String),
}

impl LocalrefError {
    /// Wrap an IO error with the path being operated on.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}
