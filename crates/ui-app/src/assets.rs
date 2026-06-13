//! Static web assets served by the Localref UI router.

use axum::response::IntoResponse;

/// Internal helper for favicon.
const FAVICON: &[u8] = include_bytes!("../../../assets/favicon.ico");
/// Internal helper for ui css.
const UI_CSS: &str = include_str!("../../../assets/localref-ui.css");
/// Internal helper for ui wasm js.
const UI_WASM_JS: &str = include_str!("../../../assets/localref-ui.js");
/// Internal helper for ui wasm bindgen js.
const UI_WASM_BINDGEN_JS: &str =
    include_str!("../../../assets/localref-ui-bindgen.js");
/// Internal helper for ui wasm.
const UI_WASM: &[u8] =
    include_bytes!("../../../assets/localref-ui-bindgen_bg.wasm");

/// Return the Localref browser favicon.
pub async fn favicon() -> impl IntoResponse {
    ([("content-type", "image/x-icon")], FAVICON)
}

/// Return the compiled Tailwind CSS used by the Localref UI.
pub async fn ui_css() -> impl IntoResponse {
    ([("content-type", "text/css; charset=utf-8")], UI_CSS)
}

/// Return the generated JavaScript bootstrap for the Rust/WASM UI controller.
pub async fn ui_wasm_js() -> impl IntoResponse {
    ([("content-type", "text/javascript; charset=utf-8")], UI_WASM_JS)
}

/// Return the generated JavaScript glue for the Rust/WASM UI controller.
pub async fn ui_wasm_bindgen_js() -> impl IntoResponse {
    ([("content-type", "text/javascript; charset=utf-8")], UI_WASM_BINDGEN_JS)
}

/// Return the generated WebAssembly module for the Rust/WASM UI controller.
pub async fn ui_wasm() -> impl IntoResponse {
    ([("content-type", "application/wasm")], UI_WASM)
}
