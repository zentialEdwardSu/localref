//! Optional Rust helpers for building Localref plugin CLI programs.
//!
//! Localref plugins are standalone stdin/stdout CLI programs. This crate keeps
//! the older Rust trait helper available for Rust plugins, but the host only
//! depends on the CLI JSON protocol documented in `docs/plugin-cli.md`.
//!
//! # Example
//!
//! ```no_run
//! use localref_plugin_sdk::localref_plugin_main;
//! use localref_plugin_sdk::prelude::*;
//!
//! #[derive(Default)]
//! struct MyPlugin;
//!
//! impl Plugin for MyPlugin {
//!     fn name(&self) -> &str { "my-plugin" }
//!
//!     fn actions(&self) -> Vec<Action> {
//!         vec![Action::new("hello", "Say Hello").mount_action_button()]
//!     }
//!
//!     fn pages(&self) -> Vec<Page> {
//!         vec![Page::new("main", "Plugin").mount_detail_tab("main")]
//!     }
//!
//!     fn render(&self, _page: &str, state: &PluginState) -> Result<RenderOutput, String> {
//!         Ok(RenderOutput::ok(format!("<p>Hello from {}</p>", state.repo_name)))
//!     }
//!
//!     fn run(&self, _action: &str, _params: &Params, _state: &PluginState) -> Result<RunOutput, String> {
//!         Ok(RunOutput::ok("Hello, World!"))
//!     }
//! }
//!
//! localref_plugin_main!(MyPlugin);
//! ```

mod plugin;
mod runtime;
pub mod state;

pub mod prelude {
    pub use crate::plugin::{Action, Page, Plugin, RenderOutput, RunOutput};
    pub use crate::state::{Params, PluginState};
    // Re-export host types that plugins commonly need.
    pub use localref_plugin::state::{
        PluginActiveDetail, PluginCategorySummary, PluginItemSummary,
    };
}

pub use prelude::*;
pub use runtime::run;
