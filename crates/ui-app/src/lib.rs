#![recursion_limit = "256"]

//! Shared Leptos application for Localref's server render and hydration.
//!
//! The crate owns the UI component tree used by both the Axum server renderer
//! and the browser WASM entry point. Server-side code supplies a serializable
//! [`UiState`], and the hydrated browser reuses the same tree for navigation.

#![warn(unreachable_pub)]
#![deny(clippy::correctness)]
#![deny(clippy::single_call_fn)]
#![deny(clippy::complexity)]
#![warn(clippy::pedantic)]
#![warn(clippy::useless_attribute)]
#![warn(clippy::redundant_pub_crate)]
#![warn(clippy::excessive_precision)]
#![warn(clippy::missing_docs_in_private_items)]

#[cfg(feature = "ssr")]
pub mod actions;
pub mod app;
#[cfg(feature = "ssr")]
pub mod assets;
#[cfg(feature = "hydrate")]
pub mod client;
#[cfg(feature = "ssr")]
pub mod dto;
pub mod model;
pub mod route;
#[cfg(feature = "ssr")]
pub mod server;
#[cfg(feature = "ssr")]
pub mod state;

pub use model::{
    ActiveDetail, CategorySummary, EventSummary, FileEntry, ItemSummary,
    PluginButtonDef, PluginDisplayDef, PluginFieldDef, PluginMenuItemDef,
    PluginPageDef, PluginPreviewDef, PluginTabDef, RuleSummary, RulesNotice,
    UiState,
};
pub use route::RouteState;
#[cfg(feature = "ssr")]
pub use server::{
    PluginHostContext, router_with_daemon, router_with_daemon_and_repo_name,
    router_with_daemon_repo_plugins_and_context,
};

/// Render the complete server-side HTML document for one UI state.
#[cfg(feature = "ssr")]
#[must_use]
pub fn render_page(state: UiState) -> String {
    use leptos::prelude::*;

    let view = app::document(state).into_view().to_html();
    format!("<!doctype html>{view}")
}

/// Hydrate the server-rendered Localref UI in the browser.
#[cfg(feature = "hydrate")]
pub fn hydrate() -> Result<(), wasm_bindgen::JsValue> {
    client::hydrate()
}

/// Start the browser-side Localref UI controller.
#[cfg(all(feature = "hydrate", target_arch = "wasm32"))]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    hydrate()
}
