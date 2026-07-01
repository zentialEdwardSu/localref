//! Vendored rust-ui components (rust-ui.com), installed via `ui-cli`.
//!
//! This is third-party source kept close to upstream. The crate-level lints in
//! `lib.rs` (`single_call_fn`, `complexity`, pedantic, `missing_docs`, …) are
//! relaxed here so vendored code stays diffable against upstream. Our own glue
//! code in `app.rs`/`views/` keeps the full lint set.
#![allow(clippy::all)]
#![allow(clippy::pedantic)]
#![allow(clippy::single_call_fn)]
#![allow(clippy::complexity)]
#![allow(clippy::missing_docs_in_private_items)]
#![allow(clippy::missing_const_for_fn)]
#![allow(missing_docs)]
#![allow(missing_debug_implementations)]
#![allow(unused_results)]
#![allow(clippy::must_use_candidate)]

pub mod hooks;
pub mod ui;
