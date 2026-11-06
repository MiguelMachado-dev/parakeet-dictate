//! The low-level keyboard hook, on a thread that does nothing else. Windows drops a hook that answers
//! slowly, so the handler must only post and return.

use std::cell::RefCell;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

use crate::win::last_error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u32,
    pub down: bool,
    /// What SendInput stored in the event: `inject::INJECTED_TAG` for the app's own keystrokes.
    pub tag: usize,
}

type Handler = Box<dyn FnMut(KeyEvent) -> bool + Send>;

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

unsafe extern "system" fn on_key(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let data = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let message = wparam as u32;
        let key = KeyEvent { vk: data.vkCode, down: message == WM_KEYDOWN || message == WM_SYSKEYDOWN, tag: data.dwExtraInfo };
        let swallow = HANDLER.with_borrow_mut(|handler| handler.as_mut().is_some_and(|handler| handler(key)));
        if swallow {
            return 1;
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

pub struct KeyboardHook {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl KeyboardHook {
    /// Hook the keyboard. `handler` runs on the hook's thread and returns true to swallow the key.
    pub fn start(handler: impl FnMut(KeyEvent) -> bool + Send + 'static) -> Result<Self, String> {
        let (ready, started) = mpsc::channel();
        let handler: Handler = Box::new(handler);
        let thread = thread::Builder::new()
            .name("keyboard-hook".to_owned())
            .spawn(move || {
                HANDLER.set(Some(handler));
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                // Creates the thread's message queue, so stop() can reach it from the first moment.
                unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
                let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), std::ptr::null_mut(), 0) };
                if hook.is_null() {
                    let _ = ready.send(Err(last_error("SetWindowsHookExW")));
                    return;
                }
                let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
                while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                    unsafe {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                unsafe { UnhookWindowsHookEx(hook) };
            })
            .map_err(|e| e.to_string())?;
        let thread_id = started.recv().map_err(|e| e.to_string())??;
        Ok(Self { thread_id, thread: Some(thread) })
    }

    pub fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
            let _ = thread.join();
        }
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        self.stop();
    }
}
