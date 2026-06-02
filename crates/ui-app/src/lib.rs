#![recursion_limit = "256"]

//! Shared Leptos application for Localref's server render and hydration.
//!
//! The crate owns the UI component tree used by both the Axum server renderer
//! and the browser WASM entry point. Server-side code supplies a serializable
//! [`UiState`], and the hydrated browser reuses the same tree for navigation.

#[cfg(feature = "ssr")]
mod actions;
mod app;
#[cfg(feature = "ssr")]
mod assets;
#[cfg(feature = "hydrate")]
mod client;
#[cfg(feature = "ssr")]
mod dto;
mod model;
mod route;
#[cfg(feature = "ssr")]
mod server;
#[cfg(feature = "ssr")]
mod state;

pub use model::{
    ActiveDetail, CategorySummary, EventSummary, FileEntry, ItemSummary,
    RuleSummary, RulesNotice, UiState,
};
pub use route::RouteState;
#[cfg(feature = "ssr")]
pub use server::{router_with_daemon, router_with_daemon_and_repo_name};

/// Render the complete server-side HTML document for one UI state.
#[cfg(feature = "ssr")]
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
