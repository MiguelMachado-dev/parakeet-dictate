use std::sync::OnceLock;
use std::thread::sleep;
use std::time::Duration;

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, MapVirtualKeyW,
    SendInput,
};

use crate::win::{last_error, wide};

const CF_UNICODETEXT: u32 = 13;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_ESCAPE: u32 = 0x1B;
const VK_RCONTROL: u16 = 0xA3;
const VK_V: u16 = 0x56;

pub const INJECTED_TAG: usize = 0x5044;
pub const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stroke {
    pub vk: u16,
    pub up: bool,
}

fn exclude_from_history() -> u32 {
    static FORMAT: OnceLock<u32> = OnceLock::new();
    *FORMAT.get_or_init(|| unsafe { RegisterClipboardFormatW(wide("ExcludeClipboardContentFromMonitorProcessing").as_ptr()) })
}

pub fn paste(text: &str) -> Result<(), String> {
    let previous_text = get_text()?;
    set_text(Some(text), false)?;
    let our_write = unsafe { GetClipboardSequenceNumber() };
    send_keys(
        &[
            Stroke { vk: VK_CONTROL, up: false },
            Stroke { vk: VK_V, up: false },
            Stroke { vk: VK_V, up: true },
            Stroke { vk: VK_CONTROL, up: true },
        ],
        INJECTED_TAG,
    )?;
    sleep(CLIPBOARD_RESTORE_DELAY);
    if unsafe { GetClipboardSequenceNumber() } == our_write {
        set_text(previous_text.as_deref(), true)?;
    }
    Ok(())
}

pub fn send_keys(strokes: &[Stroke], tag: usize) -> Result<(), String> {
    let inputs: Vec<INPUT> = strokes
        .iter()
        .map(|stroke| {
            let mut flags: KEYBD_EVENT_FLAGS = 0;
            if stroke.up {
                flags |= KEYEVENTF_KEYUP;
            }
            if stroke.vk == VK_RCONTROL {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            let scan = unsafe { MapVirtualKeyW(stroke.vk.into(), MAPVK_VK_TO_VSC) } as u16;
            let ki = KEYBDINPUT { wVk: stroke.vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: tag };
            INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }
        })
        .collect();
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(last_error("SendInput"));
    }
    Ok(())
}

struct Clipboard;

impl Clipboard {
    fn open() -> Result<Self, String> {
        for _ in 0..50 {
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return Ok(Self);
            }
            sleep(Duration::from_millis(10));
        }
        Err(last_error("OpenClipboard"))
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

pub fn get_text() -> Result<Option<String>, String> {
    let _clipboard = Clipboard::open()?;
    unsafe {
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            return Ok(None);
        }
        let pointer = GlobalLock(handle as HGLOBAL).cast::<u16>();
        if pointer.is_null() {
            return Ok(None);
        }
        let mut length = 0;
        while *pointer.add(length) != 0 {
            length += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer, length));
        GlobalUnlock(handle as HGLOBAL);
        Ok(Some(text))
    }
}

pub fn set_text(text: Option<&str>, keep_out_of_history: bool) -> Result<(), String> {
    let _clipboard = Clipboard::open()?;
    if unsafe { EmptyClipboard() } == 0 {
        return Err(last_error("EmptyClipboard"));
    }
    let Some(text) = text else {
        return Ok(());
    };
    let units = wide(text);
    let bytes = unsafe { std::slice::from_raw_parts(units.as_ptr().cast::<u8>(), units.len() * 2) };
    set_data(CF_UNICODETEXT, bytes)?;
    if keep_out_of_history && exclude_from_history() != 0 {
        set_data(exclude_from_history(), &[0])?;
    }
    Ok(())
}

fn set_data(format: u32, data: &[u8]) -> Result<(), String> {
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, data.len());
        if handle.is_null() {
            return Err(last_error("GlobalAlloc"));
        }
        let pointer = GlobalLock(handle);
        std::ptr::copy_nonoverlapping(data.as_ptr(), pointer.cast::<u8>(), data.len());
        GlobalUnlock(handle);
        if SetClipboardData(format, handle as HANDLE).is_null() {
            let failure = last_error("SetClipboardData");
            GlobalFree(handle);
            return Err(failure);
        }
    }
    Ok(())
}
