//! Helpers for building Localref plugin CLI programs.
//!
//! A plugin is a standalone CLI. The host spawns it with
//! `run <action> --endpoint … [--selected …] [--active …] [--param k=v]`.
//! The plugin reads/writes the library through `localref-client` and prints
//! one `RunOutput` JSON envelope. The same argv runs identically from a shell.

#![warn(unreachable_pub)]
#![deny(clippy::correctness)]
#![warn(clippy::pedantic)]

pub mod runtime;

pub use localref_client::{ClientError, ItemDocument as ItemDoc, LocalrefClient};
pub use localref_plugin::RunOutput;
pub use runtime::{ActionContext, Invocation, Params, emit, parse_args};
