#![recursion_limit = "256"]

//! Shared Leptos application for Localref's server render and hydration.
//!
//! The crate owns the UI component tree used by both the Axum server renderer
//! and the browser WASM entry point. Server-side code supplies a serializable
//! [`UiState`], and the hydrated browser reuses the same tree for navigation.

mod app;
#[cfg(feature = "hydrate")]
mod client;
mod model;
mod route;

pub use model::{
    ActiveDetail, CategorySummary, EventSummary, FileEntry, ItemSummary,
    RuleSummary, RulesNotice, UiState,
};
pub use route::RouteState;

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
