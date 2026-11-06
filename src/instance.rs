//! The named kernel objects the app's processes share: the tray app's single-instance mutex and the events
//! it waits on, and how to start the tray app in the background.

use std::env;
use std::os::windows::process::CommandExt;
use std::process::Command;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, SetLastError};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, SetEvent};

use crate::win::{last_error, wide};

pub const MUTEX_NAME: &str = "Local\\parakeet-dictate";
pub const QUIT_EVENT_NAME: &str = "Local\\parakeet-dictate-quit";
pub const LAUNCH_EVENT_NAME: &str = "Local\\parakeet-dictate-launched";
pub const RELOAD_EVENT_NAME: &str = "Local\\parakeet-dictate-reload";

/// A kernel handle closed when dropped.
pub struct Handle(pub HANDLE);

// Kernel handles belong to the process, not to a thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// The mutex called `name`, or None when another process already holds it.
pub fn acquire_named(name: &str) -> Result<Option<Handle>, String> {
    unsafe {
        SetLastError(0);
        let handle = CreateMutexW(std::ptr::null(), 0, wide(name).as_ptr());
        if handle.is_null() {
            return Err(last_error("CreateMutexW"));
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return Ok(None);
        }
        Ok(Some(Handle(handle)))
    }
}

pub fn create_event(name: &str, manual_reset: bool) -> Result<Handle, String> {
    let handle = unsafe { CreateEventW(std::ptr::null(), manual_reset.into(), 0, wide(name).as_ptr()) };
    if handle.is_null() {
        return Err(last_error("CreateEventW"));
    }
    Ok(Handle(handle))
}

pub fn signal_event(name: &str) -> bool {
    unsafe {
        let handle = OpenEventW(EVENT_MODIFY_STATE, 0, wide(name).as_ptr());
        if handle.is_null() {
            return false;
        }
        SetEvent(handle);
        CloseHandle(handle);
        true
    }
}

pub fn launch_background(arguments: &[&str]) -> std::io::Result<()> {
    Command::new(env::current_exe()?).args(arguments).creation_flags(CREATE_NO_WINDOW).spawn().map(|_| ())
}
