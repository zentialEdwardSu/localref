//! Localref plugin system: discovery, invocation, and shared types.
//!
//! Plugins are standalone CLI binaries placed in a designated plugins
//! directory. The host process discovers them at startup and invokes them
//! on demand via subprocess calls — no long-running plugin services.
#![warn(unreachable_pub)]
#![deny(clippy::correctness)]
#![deny(clippy::single_call_fn)]
#![deny(clippy::complexity)]
#![warn(clippy::pedantic)]
#![warn(clippy::useless_attribute)]
#![warn(clippy::redundant_pub_crate)]
#![warn(clippy::excessive_precision)]
#![warn(clippy::missing_docs_in_private_items)]

pub mod discovery;
pub mod error;
pub mod invoke;
pub mod manifest;
pub mod state;

pub use discovery::{DiscoveredPlugin, discover_plugins};
pub use error::PluginError;
pub use invoke::{invoke_render, invoke_run};
pub use manifest::{
    FieldKind, PluginManifest, PluginUiSpec, PreviewSpec, UiAction, UiDisplay,
    UiField, UiMount, UiPage, UiTarget,
};
pub use state::{
    PluginActiveDetail, PluginCategorySummary, PluginItemSummary,
    PluginUiState, RenderOutput, RunOutput,
};
