//! The tray icon itself: a hidden window that owns the notification-area entry and shows the menu.
//! Other threads post to it; every Windows call on the icon runs on the thread that pumps `run`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use log::{error, warn};
use windows_sys::Win32::Foundation::{ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BI_BITFIELDS, BITMAPINFO, BITMAPV5HEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, GetDC, HBITMAP, ReleaseDC,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW,
    GWLP_USERDATA, GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, HICON, HMENU, ICONINFO, MENUITEMINFOW, MF_CHECKED, MF_GRAYED,
    MF_POPUP, MF_SEPARATOR, MF_STRING, MFT_RADIOCHECK, MFT_STRING, MIIM_FTYPE, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SM_CXSMICON, SetForegroundWindow, SetMenuDefaultItem, SetMenuItemInfoW, SetWindowLongPtrW, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSEXW,
};

use crate::tray::{Action, Glyph, MenuItem, Tray, TrayShell};
use crate::win::{last_error, wide};

const CLASS_NAME: &str = "ParakeetDictateTray";
const ICON_ID: u32 = 1;
const WM_ICON: u32 = WM_APP + 1;
const WM_SHOW: u32 = WM_APP + 2;
const WM_BALLOON: u32 = WM_APP + 3;
const WM_STOP: u32 = WM_APP + 4;

/// Coverage of a pixel whose center lies `distance` beyond an edge, for an antialiased circle.
fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

/// The glyph's straight-alpha BGRA pixels, top row first, in the proportions of the 64-pixel originals.
pub fn glyph_pixels(glyph: Glyph, size: usize) -> Vec<u8> {
    let scale = size as f32 / 64.0;
    let center = size as f32 / 2.0;
    let (color, outer, inner) = match glyph {
        Glyph::Disc(color) => (color, 26.0 * scale, None),
        Glyph::Ring(color) => (color, 24.0 * scale, Some(16.0 * scale)),
    };
    let [_, red, green, blue] = color.to_be_bytes();
    let mut pixels = Vec::with_capacity(size * size * 4);
    for row in 0..size {
        for column in 0..size {
            let (x, y) = (column as f32 + 0.5 - center, row as f32 + 0.5 - center);
            let distance = x.hypot(y);
            let hole = inner.map_or(1.0, |inner| coverage(inner - distance));
            let alpha = coverage(distance - outer) * hole;
            pixels.extend_from_slice(&[blue, green, red, (alpha * 255.0).round() as u8]);
        }
    }
    pixels
}

unsafe fn create_icon(glyph: Glyph) -> HICON {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).max(16);
        let mut header: BITMAPV5HEADER = std::mem::zeroed();
        header.bV5Size = size_of::<BITMAPV5HEADER>() as u32;
        header.bV5Width = size;
        header.bV5Height = -size;
        header.bV5Planes = 1;
        header.bV5BitCount = 32;
        header.bV5Compression = BI_BITFIELDS;
        header.bV5RedMask = 0x00ff_0000;
        header.bV5GreenMask = 0x0000_ff00;
        header.bV5BlueMask = 0x0000_00ff;
        header.bV5AlphaMask = 0xff00_0000;
        let mut bits = std::ptr::null_mut();
        let screen = GetDC(std::ptr::null_mut());
        let info = (&raw const header).cast::<BITMAPINFO>();
        let color: HBITMAP = CreateDIBSection(screen, info, DIB_RGB_COLORS, &raw mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(std::ptr::null_mut(), screen);
        if color.is_null() {
            return std::ptr::null_mut();
        }
        let pixels = glyph_pixels(glyph, size as usize);
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
        let mask = CreateBitmap(size, size, 1, 1, std::ptr::null());
        let description = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&raw const description);
        DeleteObject(mask.cast());
        DeleteObject(color.cast());
        icon
    }
}

fn copy_text(target: &mut [u16], text: &str) {
    let limit = target.len() - 1;
    for (slot, unit) in target.iter_mut().zip(text.encode_utf16().take(limit)) {
        *slot = unit;
    }
}

struct Showing {
    glyph: Glyph,
    title: String,
}

/// What the window's thread reads. Created before the window, and alive until the process ends.
struct Shared {
    showing: Mutex<Showing>,
    balloons: Mutex<VecDeque<String>>,
    tray: OnceLock<Arc<Tray>>,
    icons: Mutex<HashMap<Glyph, usize>>,
    taskbar_created: u32,
}

impl Shared {
    fn icon(&self, glyph: Glyph) -> HICON {
        let mut icons = self.icons.lock().unwrap();
        // Handles fit in usize; the map keeps them for the life of the process, one for each color.
        *icons.entry(glyph).or_insert_with(|| unsafe { create_icon(glyph) } as usize) as HICON
    }

    fn entry(&self, hwnd: HWND) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = ICON_ID;
        data
    }

    fn apply(&self, hwnd: HWND, command: u32) {
        let (glyph, title) = {
            let showing = self.showing.lock().unwrap();
            (showing.glyph, showing.title.clone())
        };
        let mut data = self.entry(hwnd);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_ICON;
        data.hIcon = self.icon(glyph);
        copy_text(&mut data.szTip, &title);
        if unsafe { Shell_NotifyIconW(command, &raw const data) } == 0 {
            warn!("the tray icon was not updated");
        }
    }

    fn balloon(&self, hwnd: HWND, message: &str) {
        let mut data = self.entry(hwnd);
        data.uFlags = NIF_INFO;
        copy_text(&mut data.szInfo, message);
        copy_text(&mut data.szInfoTitle, "Parakeet Dictate");
        // A notification that fails must not stop the app; the message is about the app, never a dictation.
        if unsafe { Shell_NotifyIconW(NIM_MODIFY, &raw const data) } == 0 {
            error!("could not show the notification {message:?}");
        }
    }

    fn open_menu(&self, hwnd: HWND) {
        let Some(tray) = self.tray.get() else {
            return;
        };
        let mut actions = Vec::new();
        let chosen = unsafe {
            let menu = build_menu(&tray.menu(), &mut actions);
            let mut cursor = POINT { x: 0, y: 0 };
            GetCursorPos(&raw mut cursor);
            // Without the foreground the menu would stay open after a click elsewhere.
            SetForegroundWindow(hwnd);
            let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, cursor.x, cursor.y, 0, hwnd, std::ptr::null());
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            chosen
        };
        if chosen > 0 {
            tray.perform(actions[chosen as usize - 1]);
        }
    }

    fn default_action(&self) {
        if let Some(tray) = self.tray.get() {
            tray.perform(Action::Settings);
        }
    }
}

/// Command ids are positions in `actions`, plus one because 0 means the menu closed unused.
unsafe fn build_menu(items: &[MenuItem], actions: &mut Vec<Action>) -> HMENU {
    unsafe {
        let menu = CreatePopupMenu();
        for item in items {
            if item.is_separator() {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                continue;
            }
            let text = wide(&item.text);
            if !item.submenu.is_empty() {
                let submenu = build_menu(&item.submenu, actions);
                AppendMenuW(menu, MF_POPUP | MF_STRING, submenu as usize, text.as_ptr());
                continue;
            }
            let Some(action) = item.action else {
                continue;
            };
            actions.push(action);
            let id = actions.len() as u32;
            let mut flags = MF_STRING;
            if item.checked {
                flags |= MF_CHECKED;
            }
            if !item.enabled {
                flags |= MF_GRAYED;
            }
            AppendMenuW(menu, flags, id as usize, text.as_ptr());
            if item.radio {
                let mut info: MENUITEMINFOW = std::mem::zeroed();
                info.cbSize = size_of::<MENUITEMINFOW>() as u32;
                info.fMask = MIIM_FTYPE;
                info.fType = MFT_STRING | MFT_RADIOCHECK;
                SetMenuItemInfoW(menu, id, 0, &raw const info);
            }
            if item.default {
                SetMenuDefaultItem(menu, id, 0);
            }
        }
        menu
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let shared = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Shared;
    let Some(shared) = (unsafe { shared.as_ref() }) else {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    };
    match message {
        WM_ICON => match lparam as u32 & 0xffff {
            WM_LBUTTONUP => shared.default_action(),
            WM_RBUTTONUP => shared.open_menu(hwnd),
            _ => {}
        },
        WM_SHOW => shared.apply(hwnd, NIM_MODIFY),
        WM_BALLOON => {
            while let Some(message) = shared.balloons.lock().unwrap().pop_front() {
                shared.balloon(hwnd, &message);
            }
        }
        WM_STOP => unsafe {
            DestroyWindow(hwnd);
        },
        WM_DESTROY => {
            let data = shared.entry(hwnd);
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &raw const data);
                PostQuitMessage(0);
            }
        }
        // Explorer restarted: the notification area is new and our icon is gone.
        _ if message == shared.taskbar_created => shared.apply(hwnd, NIM_ADD),
        _ => return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
    0
}

/// The hidden window. Create it and `run` it on one thread; `show`, `notify` and `stop` work from any.
pub struct TrayWindow {
    hwnd: usize,
    shared: Arc<Shared>,
}

impl TrayWindow {
    pub fn new() -> Result<Self, String> {
        let shared = Arc::new(Shared {
            showing: Mutex::new(Showing { glyph: Glyph::Ring(0x9e9e9e), title: String::new() }),
            balloons: Mutex::new(VecDeque::new()),
            tray: OnceLock::new(),
            icons: Mutex::new(HashMap::new()),
            taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
        });
        let class_name = wide(CLASS_NAME);
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let mut description: WNDCLASSEXW = std::mem::zeroed();
            description.cbSize = size_of::<WNDCLASSEXW>() as u32;
            description.lpfnWndProc = Some(window_proc);
            description.hInstance = instance;
            description.lpszClassName = class_name.as_ptr();
            if RegisterClassExW(&raw const description) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(last_error("RegisterClassExW"));
            }
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                wide("Parakeet Dictate").as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return Err(last_error("CreateWindowExW"));
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Arc::as_ptr(&shared) as isize);
            Ok(Self { hwnd: hwnd as usize, shared })
        }
    }

    fn post(&self, message: u32) {
        unsafe { PostMessageW(self.hwnd as HWND, message, 0, 0) };
    }

    /// Add the icon and pump until `stop`. The menu needs `tray`, which needs this window to exist first.
    pub fn run(&self, tray: Arc<Tray>) {
        let _ = self.shared.tray.set(tray);
        self.shared.apply(self.hwnd as HWND, NIM_ADD);
        unsafe {
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }
}

impl TrayShell for TrayWindow {
    fn show(&self, glyph: Glyph, title: &str) {
        *self.shared.showing.lock().unwrap() = Showing { glyph, title: title.to_owned() };
        self.post(WM_SHOW);
    }

    fn notify(&self, message: &str) {
        self.shared.balloons.lock().unwrap().push_back(message.to_owned());
        self.post(WM_BALLOON);
    }

    fn stop(&self) {
        self.post(WM_STOP);
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        for icon in self.icons.lock().unwrap().values() {
            unsafe { DestroyIcon(*icon as HICON) };
        }
    }
}
