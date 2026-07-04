//! NTFS directory junction creation via direct Win32 FFI (`windows-sys`).
//!
//! A category link in Localref is an NTFS mount-point reparse point (a
//! "junction") from `Cat/<category>/<name>` to an `All/<item>/` directory. This
//! reimplements, in pure Rust, the `FSCTL_SET_REPARSE_POINT` sequence that the
//! former C++ `native-win32` layer performed: create the link directory, open it
//! with `FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS`, and write a
//! `MOUNT_POINT` reparse buffer naming the (canonicalized) target.
//!
//! Non-Windows builds symlink instead and never call into this module.

use std::path::Path;

use crate::error::{LocalrefError, Result};

/// Create one NTFS directory junction from `link` to `target`.
///
/// `target` is canonicalized first (junctions require a fully-qualified path).
/// On success `link` is a directory that transparently resolves to `target`.
///
/// # Errors
///
/// Returns [`LocalrefError::Platform`] when any Win32 step fails, or
/// [`LocalrefError::io`] when the target cannot be canonicalized.
#[cfg(windows)]
#[allow(clippy::single_call_fn)] // the sole Windows entry point for junctions
pub(super) fn create_directory_junction(
    link: &Path,
    target: &Path,
) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    let target = target
        .canonicalize()
        .map_err(|source| LocalrefError::io(target, source))?;
    // Print name is the plain (possibly `\\?\`-prefixed) path; the substitute
    // name is the NT-namespace form `\??\<path>` the volume actually stores.
    let print_name: Vec<u16> = target.as_os_str().encode_wide().collect();
    let substitute_name = nt_substitute_name(&print_name);

    let link_wide: Vec<u16> =
        link.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

    // SAFETY: `link_wide` is a NUL-terminated UTF-16 string; the call only reads
    // it and creates a directory. `ERROR_ALREADY_EXISTS` is tolerated because we
    // may be attaching the reparse point to a directory we just created.
    let created = unsafe {
        windows_sys::Win32::Storage::FileSystem::CreateDirectoryW(
            link_wide.as_ptr(),
            std::ptr::null(),
        )
    };
    if created == 0 {
        // SAFETY: no arguments; reads the calling thread's last-error slot.
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if code != windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS {
            return Err(platform_error("create link directory", code));
        }
    }

    let handle = open_reparse_handle(&link_wide)?;
    let result = write_mount_point(handle, &substitute_name, &print_name);
    // SAFETY: `handle` is a valid open handle returned by `CreateFileW`.
    unsafe {
        let _ = windows_sys::Win32::Foundation::CloseHandle(handle);
    }
    result
}

/// Non-Windows builds never create junctions; category links use symlinks.
#[cfg(not(windows))]
pub(super) fn create_directory_junction(
    _link: &Path,
    _target: &Path,
) -> Result<()> {
    Err(LocalrefError::Unsupported(
        "directory junctions are only supported on Windows",
    ))
}

/// Prefix a UTF-16 path with the NT object-namespace form `\??\`, collapsing an
/// existing `\\?\` win32 long-path prefix into it.
#[cfg(windows)]
#[allow(clippy::single_call_fn)] // extracted for readability of the FFI dance
fn nt_substitute_name(print_name: &[u16]) -> Vec<u16> {
    // UTF-16 for "\\?\" and "\??\".
    const WIN32_PREFIX: [u16; 4] = [0x005C, 0x005C, 0x003F, 0x005C];
    const NT_PREFIX: [u16; 4] = [0x005C, 0x003F, 0x003F, 0x005C];
    let mut out = Vec::with_capacity(print_name.len() + 4);
    out.extend_from_slice(&NT_PREFIX);
    if print_name.starts_with(&WIN32_PREFIX) {
        out.extend_from_slice(&print_name[4..]);
    } else {
        out.extend_from_slice(print_name);
    }
    out
}

/// Open `link` for reparse-point manipulation.
#[cfg(windows)]
#[allow(clippy::single_call_fn)] // extracted for readability of the FFI dance
fn open_reparse_handle(
    link_wide: &[u16],
) -> Result<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::Foundation::{
        GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        OPEN_EXISTING,
    };

    // SAFETY: `link_wide` is a NUL-terminated UTF-16 string; all other pointer
    // arguments are null as permitted by the API for this open mode.
    let handle = unsafe {
        CreateFileW(
            link_wide.as_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: no arguments; reads the calling thread's last-error slot.
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(platform_error("open reparse handle", code));
    }
    Ok(handle)
}

/// Build and write the `MOUNT_POINT` reparse buffer via `DeviceIoControl`.
#[cfg(windows)]
#[allow(clippy::single_call_fn)] // extracted for readability of the FFI dance
fn write_mount_point(
    handle: windows_sys::Win32::Foundation::HANDLE,
    substitute_name: &[u16],
    print_name: &[u16],
) -> Result<()> {
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;

    // IO_REPARSE_TAG_MOUNT_POINT. `windows-sys` does not export this constant in
    // a stable module path, so it is spelled out here (see winnt.h).
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    // Fixed header ahead of PathBuffer: ReparseTag(4) + ReparseDataLength(2) +
    // Reserved(2) + the four USHORT name offset/length fields (8) = 16 bytes.
    const HEADER_BYTES: usize = 16;
    // Bytes preceding PathBuffer *within* the reparse data payload: the four
    // USHORT fields = 8 bytes.
    const PATH_INFO_BYTES: u16 = 8;

    let wide = size_of::<u16>();
    // Name byte lengths are `USHORT` fields; reject anything that would overflow
    // one before it can corrupt the buffer layout.
    let too_long = || {
        LocalrefError::Platform("junction target path too long".to_string())
    };
    let sub_bytes = u16::try_from(std::mem::size_of_val(substitute_name))
        .map_err(|_| too_long())?;
    let print_bytes = u16::try_from(std::mem::size_of_val(print_name))
        .map_err(|_| too_long())?;
    let nul = u16::try_from(wide).map_err(|_| too_long())?;
    // Layout: [substitute][NUL][print][NUL], matching the C++ implementation.
    let path_buffer_bytes = sub_bytes
        .checked_add(nul)
        .and_then(|n| n.checked_add(print_bytes))
        .and_then(|n| n.checked_add(nul))
        .ok_or_else(too_long)?;
    let total = HEADER_BYTES + usize::from(path_buffer_bytes);

    let mut buffer = vec![0u8; total];
    let write_u16 = |buf: &mut [u8], offset: usize, value: u16| {
        buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    };
    let write_u32 = |buf: &mut [u8], offset: usize, value: u32| {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    };

    write_u32(&mut buffer, 0, IO_REPARSE_TAG_MOUNT_POINT);
    // ReparseDataLength = the four USHORT fields plus the path buffer bytes.
    let reparse_data_length =
        path_buffer_bytes.checked_add(PATH_INFO_BYTES).ok_or_else(too_long)?;
    write_u16(&mut buffer, 4, reparse_data_length);
    // Reserved (offset 6) stays zero.
    // SubstituteNameOffset = 0.
    write_u16(&mut buffer, 8, 0);
    write_u16(&mut buffer, 10, sub_bytes);
    // PrintNameOffset sits just past the substitute name and its NUL.
    write_u16(&mut buffer, 12, sub_bytes + nul);
    write_u16(&mut buffer, 14, print_bytes);

    // PathBuffer: substitute name, NUL, print name, NUL.
    let mut cursor = HEADER_BYTES;
    for unit in substitute_name {
        write_u16(&mut buffer, cursor, *unit);
        cursor += wide;
    }
    cursor += wide; // NUL terminator
    for unit in print_name {
        write_u16(&mut buffer, cursor, *unit);
        cursor += wide;
    }

    let mut returned = 0u32;
    // `total` fits in `u32` because both name lengths are `u16`-bounded above.
    let total_len = u32::try_from(total).map_err(|_| too_long())?;
    // SAFETY: `handle` is a valid reparse-capable handle; `buffer` is `total`
    // bytes long and formatted as a REPARSE_DATA_BUFFER for a mount point.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            total_len,
            std::ptr::null_mut(),
            0,
            std::ptr::from_mut(&mut returned),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        // SAFETY: no arguments; reads the calling thread's last-error slot.
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(platform_error("set reparse point", code));
    }
    Ok(())
}

/// Build a [`LocalrefError::Platform`] from a Win32 operation and error code.
#[cfg(windows)]
fn platform_error(operation: &str, code: u32) -> LocalrefError {
    LocalrefError::Platform(format!("{operation} failed: win32 error {code}"))
}

#[cfg(all(windows, test))]
mod tests {
    use super::create_directory_junction;

    #[test]
    fn junction_resolves_target_contents() {
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
}
