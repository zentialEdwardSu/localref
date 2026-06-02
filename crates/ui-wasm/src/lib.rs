//! Browser-side Rust/WASM controller for the Localref web UI.

/// Start the browser-side Localref UI controller.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    ui_app::hydrate()
}
