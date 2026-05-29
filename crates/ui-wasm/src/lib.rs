//! Browser-side Rust/WASM controller for the Localref web UI.

pub mod api;
#[cfg(target_arch = "wasm32")]
pub mod controller;
pub mod query;
pub mod render;

/// Start the browser-side Localref UI controller.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    controller::start_controller()
}
