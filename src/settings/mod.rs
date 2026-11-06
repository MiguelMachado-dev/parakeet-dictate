//! The Settings window's process: one at a time, and it tells the tray app to reload after a save.

pub mod form;
mod window;

use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, SetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MessageBoxW};

use crate::instance::{self, Handle, RELOAD_EVENT_NAME, create_event, signal_event};
use crate::win::wide;

const SETTINGS_MUTEX: &str = "Local\\parakeet-dictate-settings";
const SHOW_EVENT: &str = "Local\\parakeet-dictate-show-settings";

/// Ask the running app to reload through `reload_event`, or start it when none runs.
pub fn apply_settings(reload_event: &str, launch: impl FnOnce(&[&str]) -> io::Result<()>) -> io::Result<String> {
    if signal_event(reload_event) {
        return Ok("Saved. The app will reload after the current dictation finishes.".to_owned());
    }
    launch(&["--notify-ready"])?;
    Ok("Saved. Starting Parakeet Dictate with these settings.".to_owned())
}

pub fn main() -> i32 {
    let mutex = unsafe {
        SetLastError(0);
        CreateMutexW(std::ptr::null(), 1, wide(SETTINGS_MUTEX).as_ptr())
    };
    if mutex.is_null() {
        return 1;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(mutex) };
        signal_event(SHOW_EVENT);
        return 0;
    }
    let _mutex = Handle(mutex);
    let shown = create_event(SHOW_EVENT, false).and_then(|show| {
        window::run(&show, Box::new(|| apply_settings(RELOAD_EVENT_NAME, instance::launch_background)))
    });
    match shown {
        Ok(()) => 0,
        Err(e) => {
            let (text, title) = (wide(&e), wide("Parakeet Dictate settings"));
            unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_ICONERROR) };
            1
        }
    }
}
