//! Small helpers around the Windows API.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::GetLastError;

/// NUL-terminated UTF-16, the form every wide Windows call takes.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

pub fn wide_path(path: &Path) -> Vec<u16> {
    OsStr::new(path).encode_wide().chain(Some(0)).collect()
}

pub fn last_error(call: &str) -> String {
    let code = unsafe { GetLastError() };
    format!("{call} failed: {}", std::io::Error::from_raw_os_error(code as i32))
}

/// The text of a fixed UTF-16 buffer up to its first NUL.
pub fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}
