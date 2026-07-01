//! View components split into separate files for maintainability.
//! Each file renders one major UI block using the vendored rust-ui components.

pub mod context_menu;
pub mod detail;
pub mod files;
pub mod metadata;
pub mod plugins;
pub mod plugins_admin;
pub mod rules;
pub mod sidebar;
pub mod topbar;

use leptos::prelude::*;

use crate::model::UiState;
use crate::route::RouteState;

// ── Bridge functions (cfg-dispatch wrappers) ──────────────────────
// Views call these via `super::visit_route(...)` etc.

/// Visit a route in the hydrated browser; no-op during SSR.
pub(crate) fn visit_route(
    route: RouteState,
    set_state: WriteSignal<UiState>,
    push_history: bool,
) {
    #[cfg(feature = "hydrate")]
    crate::client::visit_route(route, set_state, push_history);

    #[cfg(not(feature = "hydrate"))]
    let _ = (route, set_state, push_history);
}

/// Submit a route action in the hydrated browser; no-op during SSR.
pub(crate) fn submit_action(
    event: leptos::ev::SubmitEvent,
    set_state: WriteSignal<UiState>,
) {
    #[cfg(feature = "hydrate")]
    crate::client::submit_action(event, set_state);

    #[cfg(not(feature = "hydrate"))]
    let _ = (event, set_state);
}

/// Submit a form when a non-submit control changes in hydrated browsers.
pub(crate) fn submit_changed_form(
    event: leptos::ev::Event,
    set_state: WriteSignal<UiState>,
) {
    #[cfg(feature = "hydrate")]
    crate::client::submit_changed_form(event, set_state);

    #[cfg(not(feature = "hydrate"))]
    let _ = (event, set_state);
}

/// Upload files selected from the hidden file input.
pub(crate) fn upload_input_files(
    event: leptos::ev::Event,
    item_id: String,
    return_to: String,
    set_state: WriteSignal<UiState>,
) {
    #[cfg(feature = "hydrate")]
    crate::client::upload_input_files(event, item_id, return_to, set_state);

    #[cfg(not(feature = "hydrate"))]
    let _ = (event, item_id, return_to, set_state);
}

/// Upload files dropped on the files pane.
pub(crate) fn upload_dropped_files(
    event: leptos::ev::DragEvent,
    item_id: String,
    return_to: String,
    set_state: WriteSignal<UiState>,
) {
    #[cfg(feature = "hydrate")]
    crate::client::upload_dropped_files(event, item_id, return_to, set_state);

    #[cfg(not(feature = "hydrate"))]
    let _ = (event, item_id, return_to, set_state);
}
