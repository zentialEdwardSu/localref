//! Safe Rust entry points for Localref's Windows-native helpers.
//!
//! Windows builds call a small C++ layer that owns Win32 and Windows App SDK
//! interaction. Non-Windows builds expose the same API and return explicit
//! unsupported errors so callers can compile without platform branches.

use std::ffi::CString;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;

/// Native helper result type.
pub type Result<T> = std::result::Result<T, NativeWin32Error>;

/// Severity used by Windows app notifications.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationKind {
    /// Informational notification.
    Info,
    /// Successful command notification.
    Success,
    /// Error notification.
    Error,
}

impl NotificationKind {
    fn as_i32(self) -> i32 {
        match self {
            Self::Info => 0,
            Self::Success => 1,
            Self::Error => 2,
        }
    }
}

/// Error returned by the native Windows helper layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeWin32Error {
    /// The operation is intentionally unavailable on this platform or build.
    Unsupported(&'static str),
    /// A Win32-style operation failed with a numeric code.
    Native { operation: &'static str, code: u32 },
    /// The user cancelled a native picker.
    Cancelled(&'static str),
    /// Input could not be passed across the native boundary.
    InvalidInput(String),
}

impl fmt::Display for NativeWin32Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(operation) => {
                write!(formatter, "unsupported native operation: {operation}")
            }
            Self::Native { operation, code } => {
                write!(
                    formatter,
                    "native operation failed: {operation}: {code}"
                )
            }
            Self::Cancelled(operation) => {
                write!(formatter, "native operation cancelled: {operation}")
            }
            Self::InvalidInput(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for NativeWin32Error {}

/// Open a file or directory through the Windows shell.
pub fn open_path(path: &Path) -> Result<()> {
    let value = path_to_string(path)?;
    call_utf8("open_path", value.as_str(), native_open_path)
}

/// Open a URI through the Windows shell.
pub fn open_uri(uri: &str) -> Result<()> {
    call_utf8("open_uri", uri, native_open_uri)
}

/// Ask the user for a file path where Localref should save generated content.
///
/// `suggested_filename` is prefilled in the native dialog, but the user can
/// edit the final file name and location before accepting.
pub fn save_file_path(suggested_filename: &str) -> Result<Option<PathBuf>> {
    let filename = c_string(suggested_filename)?;
    let mut buffer = vec![0 as std::ffi::c_char; 32_768];
    let code = unsafe {
        native_save_file_dialog(
            filename.as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
        )
    };
    match code {
        0 => {
            let path = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
                .to_str()
                .map_err(|error| {
                    NativeWin32Error::InvalidInput(error.to_string())
                })?;
            Ok(Some(PathBuf::from(path)))
        }
        1223 => Ok(None),
        50 => Err(NativeWin32Error::Unsupported("save_file_path")),
        code => {
            Err(NativeWin32Error::Native { operation: "save_file_path", code })
        }
    }
}

/// Create one NTFS directory junction.
pub fn create_directory_junction(link: &Path, target: &Path) -> Result<()> {
    let link = path_to_string(link)?;
    let target = target
        .canonicalize()
        .map_err(|error| NativeWin32Error::InvalidInput(error.to_string()))?;
    let target = path_to_string(&target)?;
    let link = c_string(&link)?;
    let target = c_string(&target)?;
    native_result("create_directory_junction", unsafe {
        native_create_directory_junction(link.as_ptr(), target.as_ptr())
    })
}

/// Register this process for Windows App SDK app notifications.
pub fn register_app_notifications() -> Result<()> {
    native_result("register_app_notifications", unsafe {
        native_register_app_notifications()
    })
}

/// Register this process for app notifications using a shortcut icon.
pub fn register_app_notifications_with_icon(icon_path: &Path) -> Result<()> {
    let icon_path = path_to_string(icon_path)?;
    let icon_path = c_string(&icon_path)?;
    native_result("register_app_notifications_with_icon", unsafe {
        native_register_app_notifications_with_icon(icon_path.as_ptr())
    })
}

/// Unregister this process from Windows App SDK app notifications.
pub fn unregister_app_notifications() -> Result<()> {
    native_result("unregister_app_notifications", unsafe {
        native_unregister_app_notifications()
    })
}

/// Show one local Windows app notification.
pub fn show_app_notification(
    title: &str,
    body: &str,
    kind: NotificationKind,
) -> Result<()> {
    let title = c_string(title)?;
    let body = c_string(body)?;
    native_result("show_app_notification", unsafe {
        native_show_app_notification(
            title.as_ptr(),
            body.as_ptr(),
            kind.as_i32(),
        )
    })
}

/// Return whether app notification native support was compiled in.
pub fn app_notifications_available() -> bool {
    unsafe { native_app_notifications_available() != 0 }
}

/// Detach the inherited Windows console for quiet desktop startup.
pub fn detach_console() -> Result<()> {
    native_result("detach_console", unsafe { native_detach_console() })
}

fn call_utf8(
    operation: &'static str,
    value: &str,
    callback: unsafe extern "C" fn(*const std::ffi::c_char) -> u32,
) -> Result<()> {
    let value = c_string(value)?;
    native_result(operation, unsafe { callback(value.as_ptr()) })
}

fn c_string(value: &str) -> Result<CString> {
    CString::new(value).map_err(|_| {
        NativeWin32Error::InvalidInput("native input contains NUL".to_string())
    })
}

fn path_to_string(path: &Path) -> Result<String> {
    path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
        NativeWin32Error::InvalidInput("path is not valid UTF-8".to_string())
    })
}

#[cfg(windows)]
fn native_result(operation: &'static str, code: u32) -> Result<()> {
    match code {
        0 => Ok(()),
        50 => Err(NativeWin32Error::Unsupported(operation)),
        code => Err(NativeWin32Error::Native { operation, code }),
    }
}

#[cfg(not(windows))]
fn native_result(operation: &'static str, code: u32) -> Result<()> {
    let _ = code;
    Err(NativeWin32Error::Unsupported(operation))
}

#[cfg(windows)]
unsafe extern "C" {
    fn native_open_path(path: *const std::ffi::c_char) -> u32;
    fn native_open_uri(uri: *const std::ffi::c_char) -> u32;
    fn native_save_file_dialog(
        default_filename: *const std::ffi::c_char,
        out_path: *mut std::ffi::c_char,
        out_path_len: usize,
    ) -> u32;
    fn native_create_directory_junction(
        link: *const std::ffi::c_char,
        target: *const std::ffi::c_char,
    ) -> u32;
    fn native_register_app_notifications() -> u32;
    fn native_register_app_notifications_with_icon(
        icon_path: *const std::ffi::c_char,
    ) -> u32;
    fn native_unregister_app_notifications() -> u32;
    fn native_show_app_notification(
        title: *const std::ffi::c_char,
        body: *const std::ffi::c_char,
        kind: i32,
    ) -> u32;
    fn native_app_notifications_available() -> i32;
    fn native_detach_console() -> u32;
}

#[cfg(not(windows))]
unsafe extern "C" fn native_open_path(_: *const std::ffi::c_char) -> u32 {
    50
}

#[cfg(not(windows))]
unsafe extern "C" fn native_open_uri(_: *const std::ffi::c_char) -> u32 {
    50
}

#[cfg(not(windows))]
unsafe extern "C" fn native_save_file_dialog(
    _: *const std::ffi::c_char,
    _: *mut std::ffi::c_char,
    _: usize,
) -> u32 {
    50
}

#[cfg(not(windows))]
unsafe extern "C" fn native_create_directory_junction(
    _: *const std::ffi::c_char,
    _: *const std::ffi::c_char,
) -> u32 {
    50
}

#[cfg(not(windows))]
unsafe extern "C" fn native_register_app_notifications() -> u32 {
    50
}

#[cfg(not(windows))]
unsafe extern "C" fn native_register_app_notifications_with_icon(
    _: *const std::ffi::c_char,
) -> u32 {
    50
}

#[cfg(not(windows))]
unsafe extern "C" fn native_unregister_app_notifications() -> u32 {
    50
}

#[cfg(not(windows))]
unsafe extern "C" fn native_show_app_notification(
    _: *const std::ffi::c_char,
    _: *const std::ffi::c_char,
    _: i32,
) -> u32 {
    50
}

#[cfg(not(windows))]
unsafe extern "C" fn native_app_notifications_available() -> i32 {
    0
}

#[cfg(not(windows))]
unsafe extern "C" fn native_detach_console() -> u32 {
    50
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nul_input_before_native_boundary() {
        assert!(matches!(
            open_uri("http://local\0ref"),
            Err(NativeWin32Error::InvalidInput(_))
        ));
        assert!(matches!(
            save_file_path("bad\0name.bib"),
            Err(NativeWin32Error::InvalidInput(_))
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_reports_notifications_unavailable() {
        assert!(!app_notifications_available());
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_stub_returns_unsupported() {
        assert!(matches!(
            open_uri("http://127.0.0.1"),
            Err(NativeWin32Error::Unsupported(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "opens shell UI and requires a desktop session"]
    fn open_uri_smoke_test() {
        open_uri("http://127.0.0.1").unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "creates an NTFS junction and requires Windows filesystem support"]
    fn creates_directory_junction() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("paper.txt"), "ok").unwrap();

        create_directory_junction(&link, &target).unwrap();

        assert_eq!(
            std::fs::read_to_string(link.join("paper.txt")).unwrap(),
            "ok"
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "shows a Windows app notification"]
    fn app_notification_smoke_test() {
        register_app_notifications().unwrap();
        show_app_notification(
            "Localref",
            "Native notification test",
            NotificationKind::Info,
        )
        .unwrap();
        unregister_app_notifications().unwrap();
    }
}
