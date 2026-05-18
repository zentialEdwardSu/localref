//! Desktop entry support for Localref.
//!
//! The library contains REST client and view-model code used by desktop entry
//! binaries. The Dioxus binary itself is behind the `desktop` feature so normal
//! backend tests do not compile platform webview dependencies.

mod client;
mod view_model;

#[cfg(feature = "desktop")]
pub mod desktop_app;

pub use client::RestClient;
pub use localref_config::DEFAULT_REST_ENDPOINT;
pub use view_model::{
    CategoryMoveRequest, CategoryRequest, CategorySummary, DaemonStatus,
    DashboardSnapshot, MetadataDocument, MetadataPatchRequest, PauseRequest,
    PendingConfirmationRequest, PendingImportSummary,
};
