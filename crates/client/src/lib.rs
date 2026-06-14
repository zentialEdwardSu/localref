//! Typed async REST client for the Localref daemon.
//!
//! One set of methods over the daemon's REST routes, returning
//! `localref-core` model types. Used by the host and by Rust plugins.

use localref_core::model::{
    ItemDocument, ItemFilesDocument, MetadataDocument, SearchHit,
};
use localref_core::storage::CategorySummary;

/// Errors surfaced by the REST client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Transport-level failure (connect, timeout, TLS).
    #[error("request transport error: {0}")]
    Transport(String),
    /// Server returned a non-success HTTP status.
    #[error("unexpected status {status}: {body}")]
    Status {
        /// HTTP status code returned by the daemon.
        status: u16,
        /// Response body text, for diagnostics.
        body: String,
    },
    /// Response body did not deserialize into the expected type.
    #[error("decode error: {0}")]
    Decode(String),
}

/// Async REST client bound to one daemon endpoint.
#[derive(Clone, Debug)]
pub struct LocalrefClient {
    endpoint: String,
    http: reqwest::Client,
}

impl LocalrefClient {
    /// Build a client for the given REST base URL (e.g. `http://127.0.0.1:8787`).
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Return the configured base endpoint.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
