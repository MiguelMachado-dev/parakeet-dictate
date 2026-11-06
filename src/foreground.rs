use std::collections::BTreeMap;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible};
use windows_sys::core::BOOL;

use crate::text::casefold;

/// The foreground window's executable name, or None if it vanished or cannot be read.
pub fn foreground_executable() -> Option<String> {
    window_executable(unsafe { GetForegroundWindow() })
}

/// Executable names of apps with visible windows, for the rule editor.
pub fn running_executables() -> Vec<String> {
    unsafe extern "system" fn collect(hwnd: HWND, names: LPARAM) -> BOOL {
        let names = unsafe { &mut *(names as *mut BTreeMap<String, String>) };
        if unsafe { IsWindowVisible(hwnd) } != 0
            && let Some(name) = window_executable(hwnd)
        {
            names.insert(casefold(&name), name);
        }
        1
    }
    let mut names = BTreeMap::<String, String>::new();
    unsafe { EnumWindows(Some(collect), &raw mut names as LPARAM) };
    names.into_values().collect()
}

#[allow(clippy::not_unsafe_ptr_arg_deref, reason = "an HWND is an opaque handle that Windows validates; it is never dereferenced")]
pub fn window_executable(hwnd: HWND) -> Option<String> {
    let mut pid = 0;
    if hwnd.is_null() || unsafe { GetWindowThreadProcessId(hwnd, &mut pid) } == 0 || pid == 0 {
        return None;
    }
    process_executable(pid)
}

pub fn process_executable(pid: u32) -> Option<String> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let mut path = vec![0u16; 32768];
    let mut size = path.len() as u32;
    let read = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(process) };
    if read == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&path[..size as usize]);
    Path::new(&path).file_name().map(|name| name.to_string_lossy().into_owned()).filter(|name| !name.is_empty())
}
