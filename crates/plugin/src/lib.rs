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
pub mod registry;
pub mod state;

pub use discovery::{DiscoveredPlugin, discover_plugins};
pub use error::PluginError;
pub use invoke::{
    DEFAULT_PLUGIN_TIMEOUT_SECS, InvocationTracking, invoke_action,
    invoke_cron, invoke_hook,
};
pub use manifest::{
    CronJob, DisplayKind, ExtraFieldDecl, FieldKind, HookBinding, HookEvent,
    PluginManifest, PluginUiSpec, PreviewSpec, UiAction, UiConfirmation,
    UiDataRequirement, UiDisplay, UiDisplayColumn, UiField, UiMount, UiPage,
    UiSubmit, UiTarget,
};
pub use registry::{
    InvocationKind, PluginProcessRegistry, RegistrationGuard,
    RunningInvocation,
};
pub use state::{ActionArgs, HookArgs, RunOutput};
