//! The on-screen overlay: a layered Win32 window that never takes focus or input.
//!
//! It runs on its own thread with its own message loop. While something is on screen it draws at
//! FPS; otherwise the thread blocks on an event with no timer armed, so it uses no CPU.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use log::{error, warn};
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HANDLE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WAIT_FAILED, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetMonitorInfoW, HBITMAP, HDC, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromPoint, MonitorFromWindow, SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateEventW, CreateWaitableTimerExW, GetCurrentThreadId, INFINITE,
    SetEvent, SetWaitableTimer, TIMER_ALL_ACCESS,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow, HTTRANSPARENT, HWND_TOPMOST, MA_NOACTIVATE, MSG,
    MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassExW, SWP_HIDEWINDOW, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos, TranslateMessage, ULW_ALPHA, UnregisterClassW, UpdateLayeredWindow,
    WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};

use super::frame::FPS;
use super::scene::{Command, Placement, Scene, command_for, place};
use super::{Style, premultiplied_bgra};
use crate::clock;
use crate::instance::Handle;
use crate::meter::{Levels, Spectrum};
use crate::reducer::Transition;
use crate::win::{last_error, wide};

pub const CLASS_NAME: &str = "ParakeetDictateOverlay";

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The layered window and the one DIB section it is drawn from, reallocated only on a size change.
pub struct Window {
    instance: HMODULE,
    hwnd: HWND,
    dc: HDC,
    bitmap: HBITMAP,
    previous_bitmap: HGDIOBJ,
    bits: *mut u8,
    size: (i32, i32),
    visible: bool,
}

impl Window {
    pub fn new() -> Result<Self, String> {
        let class = wide(CLASS_NAME);
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let description = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            if RegisterClassExW(&description) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(last_error("RegisterClassExW"));
            }
            let style = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
            let title = wide("Parakeet Dictate overlay");
            let null = std::ptr::null_mut();
            let hwnd = CreateWindowExW(style, class.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 0, 0, null, null, instance, std::ptr::null());
            if hwnd.is_null() {
                return Err(last_error("CreateWindowExW"));
            }
            Ok(Self {
                instance,
                hwnd,
                dc: null,
                bitmap: null,
                previous_bitmap: null,
                bits: std::ptr::null_mut(),
                size: (0, 0),
                visible: false,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn draw(&mut self, rgba: &[u8], placement: Placement) -> Result<(), String> {
        let size = (placement.width, placement.height);
        if size != self.size {
            self.allocate(size)?;
        }
        let bytes = (size.0 * size.1 * 4) as usize;
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, bytes) };
        premultiplied_bgra(rgba, pixels);
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let at = POINT { x: placement.x, y: placement.y };
        let extent = SIZE { cx: size.0, cy: size.1 };
        let origin = POINT { x: 0, y: 0 };
        let updated = unsafe { UpdateLayeredWindow(self.hwnd, std::ptr::null_mut(), &at, &extent, self.dc, &origin, 0, &blend, ULW_ALPHA) };
        if updated == 0 {
            return Err(last_error("UpdateLayeredWindow"));
        }
        if !self.visible {
            // SetWindowPos rather than ShowWindow: a process's first ShowWindow call can take its
            // show command from STARTUPINFO, which would activate the window.
            unsafe { SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW) };
            self.visible = true;
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.visible {
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW;
            unsafe { SetWindowPos(self.hwnd, std::ptr::null_mut(), 0, 0, 0, 0, flags) };
            self.visible = false;
        }
    }

    fn allocate(&mut self, size: (i32, i32)) -> Result<(), String> {
        self.free();
        unsafe {
            let mut info: BITMAPINFO = std::mem::zeroed();
            // A negative height makes the rows top-down.
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size.0,
                biHeight: -size.1,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits = std::ptr::null_mut();
            self.dc = CreateCompatibleDC(std::ptr::null_mut());
            self.bitmap = CreateDIBSection(self.dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if self.dc.is_null() || self.bitmap.is_null() {
                return Err(last_error("CreateDIBSection"));
            }
            self.previous_bitmap = SelectObject(self.dc, self.bitmap);
            self.bits = bits.cast();
            self.size = size;
        }
        Ok(())
    }

    fn free(&mut self) {
        if !self.dc.is_null() {
            unsafe {
                SelectObject(self.dc, self.previous_bitmap);
                DeleteObject(self.bitmap);
                DeleteDC(self.dc);
            }
        }
        self.dc = std::ptr::null_mut();
        self.bitmap = std::ptr::null_mut();
        self.bits = std::ptr::null_mut();
        self.size = (0, 0);
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.free();
        unsafe {
            DestroyWindow(self.hwnd);
            UnregisterClassW(wide(CLASS_NAME).as_ptr(), self.instance);
        }
    }
}

/// Where a dictation shows: the monitor of the window that had focus when it started.
fn placement_for(foreground: isize) -> Placement {
    unsafe {
        let monitor = if foreground == 0 {
            MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
        } else {
            MonitorFromWindow(foreground as HWND, MONITOR_DEFAULTTONEAREST)
        };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT { left: 0, top: 0, right: 0, bottom: 0 },
            rcWork: RECT { left: 0, top: 0, right: 0, bottom: 0 },
            dwFlags: 0,
        };
        GetMonitorInfoW(monitor, &mut info);
        let (mut dpi_x, mut dpi_y) = (96, 96);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let work = info.rcWork;
        place((work.left, work.top, work.right, work.bottom), dpi_x)
    }
}

enum Item {
    Command { t: f64, command: Command, foreground: isize },
    Restyle(Option<Style>),
}

type LevelSource = Box<dyn Fn() -> (Levels, Spectrum) + Send>;

/// Where the overlay stands, read by tests and the benchmark.
#[derive(Default)]
pub struct OverlayStats {
    pub frames: AtomicU64,
    /// Frames rendered by each `Style`, indexed by `style as usize`, counted before the window shows them.
    pub style_frames: [AtomicU64; 2],
    pub hwnd: AtomicU64,
    pub thread_id: AtomicU64,
    /// The phase on screen, as `Phase as u32 + 1`, or 0 while hidden.
    pub phase: AtomicU32,
    /// The bits of the latest frame's presence, as f32.
    pub presence: AtomicU32,
}

/// The window's side of `ActiveOverlay`, so tests can run it without a window.
pub trait OverlaySurface: Send {
    fn notify(&self, transition: &Transition);
    fn restyle(&self, style: Option<Style>);
    fn close(&mut self);
}

pub struct Overlay {
    commands: Sender<Item>,
    wake: Arc<Handle>,
    closing: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub stats: Arc<OverlayStats>,
}

struct Loop {
    style: Option<Style>,
    levels: LevelSource,
    commands: Receiver<Item>,
    closing: Arc<AtomicBool>,
    stats: Arc<OverlayStats>,
}

impl Overlay {
    /// `levels` returns the current meter's levels and spectrum; the overlay thread calls it each frame.
    pub fn new(style: Style, levels: impl Fn() -> (Levels, Spectrum) + Send + 'static) -> Result<Self, String> {
        let wake = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if wake.is_null() {
            return Err(last_error("CreateEventW"));
        }
        let wake = Arc::new(Handle(wake));
        let (commands, inbox) = mpsc::channel();
        let closing = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(OverlayStats::default());
        let (ready, started) = mpsc::channel();
        let work = Loop { style: Some(style), levels: Box::new(levels), commands: inbox, closing: closing.clone(), stats: stats.clone() };
        let wake_now = wake.clone();
        let thread = thread::Builder::new()
            .name("overlay".to_owned())
            .spawn(move || work.run(&wake_now, &ready))
            .map_err(|e| e.to_string())?;
        started.recv().map_err(|e| e.to_string())??;
        Ok(Self { commands, wake, closing, thread: Some(thread), stats })
    }

    fn wake(&self) {
        unsafe { SetEvent(self.wake.0) };
    }
}

impl OverlaySurface for Overlay {
    /// Called on the controller thread: queue the command and wake the overlay thread.
    fn notify(&self, transition: &Transition) {
        if let Some(command) = command_for(transition) {
            let foreground = unsafe { GetForegroundWindow() } as isize;
            let _ = self.commands.send(Item::Command { t: clock::now(), command, foreground });
            self.wake();
        }
    }

    /// Draw with `style` from the next frame. None turns the overlay off: it hides at once, drops
    /// the dictation on screen, and ignores transitions until a style is set again.
    fn restyle(&self, style: Option<Style>) {
        let _ = self.commands.send(Item::Restyle(style));
        self.wake();
    }

    fn close(&mut self) {
        self.closing.store(true, Ordering::Release);
        self.wake();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Loop {
    fn run(self, wake: &Handle, ready: &Sender<Result<(), String>>) {
        // Per thread, so the tray and the paste keep the process's default DPI behavior.
        if unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.is_null() {
            warn!("overlay: per-monitor DPI awareness unavailable; sizes assume 100% scale");
        }
        let mut window = match Window::new() {
            Ok(window) => window,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        self.stats.hwnd.store(window.hwnd as u64, Ordering::Release);
        self.stats.thread_id.store(unsafe { GetCurrentThreadId() }.into(), Ordering::Release);
        let _ = ready.send(Ok(()));
        let timer = unsafe {
            CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS)
        };
        if timer.is_null() {
            error!("overlay stopped: {}", last_error("CreateWaitableTimerExW"));
            return;
        }
        let timer = Handle(timer);
        if let Err(e) = self.draw_until_closed(&mut window, wake.0, timer.0) {
            error!("overlay stopped: {e}");
        }
    }

    fn draw_until_closed(mut self, window: &mut Window, wake: HANDLE, timer: HANDLE) -> Result<(), String> {
        let mut scene = Scene::default();
        let mut canvas: Vec<u8> = Vec::new();
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        let handles = [wake, timer];
        let frame_s = 1.0 / FPS;
        let mut deadline: Option<f64> = None;
        while !self.closing.load(Ordering::Acquire) {
            while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            while let Ok(item) = self.commands.try_recv() {
                match item {
                    Item::Restyle(style) => {
                        self.style = style;
                        if style.is_none() {
                            scene = Scene::default();
                        }
                    }
                    Item::Command { t, command, foreground } => {
                        if self.style.is_some() {
                            scene.apply(command, t, placement_for(foreground));
                        }
                    }
                }
            }

            let now = clock::now();
            let (levels, spectrum) = (self.levels)();
            let due = match (scene.frame(now, &levels, &spectrum), self.style) {
                (Some((frame, placement)), Some(style)) => {
                    let bytes = (placement.width * placement.height * 4) as usize;
                    canvas.resize(bytes, 0);
                    style.render(&frame, &mut canvas);
                    self.stats.style_frames[style as usize].fetch_add(1, Ordering::Relaxed);
                    let (phase, presence) = (frame.phase as u32 + 1, (frame.presence as f32).to_bits());
                    window.draw(&canvas, placement)?;
                    self.stats.frames.fetch_add(1, Ordering::Relaxed);
                    self.stats.phase.store(phase, Ordering::Release);
                    self.stats.presence.store(presence, Ordering::Release);
                    let next = match deadline {
                        Some(last) if last + frame_s >= now => last + frame_s,
                        _ => now + frame_s,
                    };
                    deadline = Some(next);
                    Some(next)
                }
                _ => {
                    window.hide();
                    self.stats.phase.store(0, Ordering::Release);
                    deadline = None;
                    scene.next_show()
                }
            };

            match due {
                None => {
                    unsafe { CancelWaitableTimer(timer) };
                }
                Some(due) => {
                    let relative = -(((due - clock::now()) * 1e7).round() as i64).max(1);
                    unsafe { SetWaitableTimer(timer, &relative, 0, None, std::ptr::null(), 0) };
                }
            }
            let waited = unsafe { MsgWaitForMultipleObjectsEx(2, handles.as_ptr(), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
            if waited == WAIT_FAILED {
                return Err(last_error("MsgWaitForMultipleObjectsEx"));
            }
        }
        Ok(())
    }
}
