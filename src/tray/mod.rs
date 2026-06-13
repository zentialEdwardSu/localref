#[cfg(feature = "native-tray")]
mod native_tray;

#[path = "tray.rs"]
pub mod controller;

pub use controller::*;
