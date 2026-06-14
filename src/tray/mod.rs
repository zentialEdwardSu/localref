#[cfg(feature = "native-tray")]
mod native_tray;

pub(crate) mod rest_bridge;

#[path = "tray.rs"]
pub mod controller;

pub use controller::*;
