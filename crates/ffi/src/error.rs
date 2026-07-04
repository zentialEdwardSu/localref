//! The error type surfaced across the UniFFI boundary.
//!
//! Every fallible `DaemonHandle` method returns [`FfiError`], which UniFFI turns
//! into typed C# exceptions. `Conflict` is kept as its own variant so the UI can
//! catch a metadata revision conflict from `patch_metadata`, reload, and
//! re-prompt (mirroring the old REST 409 path) rather than silently writing a
//! `metadata.daemon.toml` sidecar.

use localref_core::error::LocalrefError;
use localref_plugin::PluginError;

/// Error returned by every fallible [`crate::DaemonHandle`] method.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    /// A filesystem operation failed.
    #[error("io error: {msg}")]
    Io {
        /// Human-readable description including the path.
        msg: String,
    },
    /// The requested item, category, or resource does not exist.
    #[error("not found: {msg}")]
    NotFound {
        /// What was missing.
        msg: String,
    },
    /// A metadata revision conflict: the UI must reload and re-prompt.
    #[error("conflict: {msg}")]
    Conflict {
        /// Conflict detail.
        msg: String,
    },
    /// Caller-supplied input was rejected (bad id, path component, etc.).
    #[error("invalid input: {msg}")]
    InvalidInput {
        /// Why the input was rejected.
        msg: String,
    },
    /// The operation is unsupported in this build or state (e.g. paused).
    #[error("unsupported: {msg}")]
    Unsupported {
        /// What is unsupported.
        msg: String,
    },
    /// A plugin invocation failed (spawn, timeout, or plugin-reported error).
    #[error("plugin error: {msg}")]
    Plugin {
        /// Plugin failure detail.
        msg: String,
    },
    /// An internal error with no more specific mapping.
    #[error("internal error: {msg}")]
    Internal {
        /// Failure detail.
        msg: String,
    },
}

impl From<LocalrefError> for FfiError {
    fn from(value: LocalrefError) -> Self {
        match value {
            LocalrefError::Io { .. } => {
                FfiError::Io { msg: value.to_string() }
            }
            LocalrefError::MissingField(field) => {
                FfiError::NotFound { msg: field.to_string() }
            }
            LocalrefError::Conflict(msg) => FfiError::Conflict { msg },
            LocalrefError::InvalidPathComponent { .. } => {
                FfiError::InvalidInput { msg: value.to_string() }
            }
            LocalrefError::Unsupported(msg) => {
                FfiError::Unsupported { msg: msg.to_string() }
            }
            LocalrefError::Json(_)
            | LocalrefError::TomlSer(_)
            | LocalrefError::TomlDe(_)
            | LocalrefError::TomlEdit(_)
            | LocalrefError::Platform(_)
            | LocalrefError::Rule(_)
            | LocalrefError::Storage(_) => {
                FfiError::Internal { msg: value.to_string() }
            }
        }
    }
}

impl From<PluginError> for FfiError {
    fn from(value: PluginError) -> Self {
        match value {
            PluginError::NotFound(msg) => FfiError::NotFound { msg },
            other => FfiError::Plugin { msg: other.to_string() },
        }
    }
}
