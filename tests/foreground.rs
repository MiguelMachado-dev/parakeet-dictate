use std::env;
use std::ptr;

use parakeet_dictate::foreground::{process_executable, window_executable};
use parakeet_dictate::text::casefold;
use parakeet_dictate::win::wide;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow};

/// A real, hidden window owned by this process; never changes the user's focus.
struct Window(HWND);

impl Window {
    fn new() -> Self {
        let (class, title) = (wide("STATIC"), wide("Parakeet app rules test"));
        let hwnd = unsafe {
            CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0, 0, 0, 1, 1, ptr::null_mut(), ptr::null_mut(), ptr::null_mut(), ptr::null())
        };
        assert!(!hwnd.is_null(), "{}", std::io::Error::last_os_error());
        Self(hwnd)
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) };
    }
}

fn handle_count() -> u32 {
    let mut count = 0;
    assert_ne!(unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) }, 0);
    count
}

#[test]
fn test_window_owner_is_resolved_through_real_win32_process_apis() {
    let window = Window::new();
    let exe = env::current_exe().unwrap();
    let name = exe.file_name().unwrap().to_string_lossy();
    assert_eq!(casefold(&window_executable(window.0).unwrap()), casefold(&name));
}

#[test]
fn test_no_foreground_window_means_no_app() {
    assert_eq!(window_executable(ptr::null_mut()), None);
}

#[test]
fn test_vanished_window_means_no_app() {
    assert_eq!(window_executable(0x1234_5678 as HWND), None);
}

#[test]
fn test_a_process_that_cannot_be_opened_means_no_app() {
    // Process ids are multiples of four below this; no process has it, so OpenProcess fails for real.
    assert_eq!(process_executable(0xFFFF_FFFC), None);
}

#[test]
fn test_the_process_handle_is_closed_even_when_its_name_cannot_be_read() {
    const CALLS: u32 = 2000;
    let window = Window::new();
    let before = handle_count();
    for _ in 0..CALLS {
        assert!(window_executable(window.0).is_some());
    }
    // A leak grows the count by one per call. Other tests in this process may hold a few handles.
    let after = handle_count();
    assert!(after < before + CALLS / 2, "handles grew from {before} to {after}");
}
