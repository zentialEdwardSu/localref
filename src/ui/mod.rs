//! Desktop entry support for Localref.
//!
//! The module contains desktop view code used by Localref UI entry points. The
//! iced desktop app is behind the `desktop` feature so normal backend tests do
//! not compile platform UI dependencies.

#[cfg(feature = "desktop")]
pub mod desktop_app;

pub use crate::rest_client::{CategorySummary, DashboardSnapshot, RestClient};
