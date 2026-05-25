//! Desktop entry support for Localref.
//!
//! The module contains REST client and view-model code used by desktop entry
//! points. The iced desktop app is behind the `desktop` feature so normal
//! backend tests do not compile platform UI dependencies.

mod client;
mod view_model;

#[cfg(feature = "desktop")]
pub mod desktop_app;

pub use client::RestClient;
pub use view_model::{CategorySummary, DashboardSnapshot};
