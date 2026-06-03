//! Localref plugin system: discovery, invocation, and shared types.
//!
//! Plugins are standalone CLI binaries placed in a designated plugins
//! directory. The host process discovers them at startup and invokes them
//! on demand via subprocess calls — no long-running plugin services.

pub mod discovery;
pub mod error;
pub mod invoke;
pub mod manifest;
pub mod state;

pub use discovery::{DiscoveredPlugin, discover_plugins};
pub use error::PluginError;
pub use invoke::{invoke_render, invoke_run};
pub use manifest::{
    ActionMount, ActionSpec, PageMount, PageSpec, PluginManifest,
};
pub use state::{
    PluginActiveDetail, PluginCategorySummary, PluginItemSummary,
    PluginUiState, RenderOutput, RunOutput,
};
