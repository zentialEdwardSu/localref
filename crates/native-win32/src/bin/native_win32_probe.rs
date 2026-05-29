//! Manual diagnostic binary for Localref Windows-native helpers.
//!
//! The probe keeps UI-affecting checks behind explicit command line flags so
//! regular test runs can verify wiring without opening shell windows or
//! notifications.

use std::env;
use std::path::PathBuf;

use native_win32::{NativeWin32Error, NotificationKind};

/// Run the native helper probe and return a process exit code.
fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return;
    }

    println!("native-win32 probe");
    println!("target_os=windows: {}", cfg!(windows));
    println!(
        "app_notifications_compiled: {}",
        native_win32::app_notifications_available()
    );

    if let Err(error) = input_validation_check() {
        eprintln!("input_validation: failed: {error}");
        std::process::exit(1);
    }
    println!("input_validation: ok");

    if args.iter().any(|arg| arg == "--notification") {
        if let Err(error) = notification_check(icon_path(&args).as_deref()) {
            eprintln!("notification: failed: {error}");
            std::process::exit(1);
        }
        println!("notification: ok");
    } else {
        println!("notification: skipped; pass --notification to show a toast");
    }
}

/// Print supported probe arguments.
fn print_help() {
    println!(
        "Usage: cargo run -p native-win32 --bin native_win32_probe -- [--notification] [--icon PATH]"
    );
    println!(
        "  --notification  register, show, and unregister a Localref toast"
    );
    println!("  --icon PATH     use PATH as the notification shortcut icon");
}

/// Verify Rust-side validation before crossing the native boundary.
fn input_validation_check() -> native_win32::Result<()> {
    match native_win32::open_uri("http://local\0ref") {
        Err(NativeWin32Error::InvalidInput(_)) => Ok(()),
        Ok(()) => Err(NativeWin32Error::InvalidInput(
            "NUL URI unexpectedly reached native boundary".to_string(),
        )),
        Err(error) => Err(error),
    }
}

/// Register and display one notification through the native helper layer.
fn notification_check(
    icon_path: Option<&std::path::Path>,
) -> native_win32::Result<()> {
    if let Some(icon_path) = icon_path {
        native_win32::register_app_notifications_with_icon(icon_path)?;
    } else {
        native_win32::register_app_notifications()?;
    }
    let result = native_win32::show_app_notification(
        "Localref native probe",
        "If this toast appears, native-win32 notification delivery is wired.",
        NotificationKind::Info,
    );
    let unregister = native_win32::unregister_app_notifications();
    result.and(unregister)
}

/// Return the optional icon path supplied to the probe.
fn icon_path(args: &[String]) -> Option<PathBuf> {
    args.windows(2)
        .find(|pair| pair[0] == "--icon")
        .map(|pair| PathBuf::from(&pair[1]))
}
