//! Port of tests/test_overlay_window.py.

use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use parakeet_dictate::clock;
use parakeet_dictate::meter::LevelMeter;
use parakeet_dictate::overlay::frame::FPS;
use parakeet_dictate::overlay::premultiplied_bgra;
use parakeet_dictate::overlay::scene::{Placement, SHOW_DELAY_S};
use parakeet_dictate::overlay::window::{Overlay, OverlaySurface, Window};
use parakeet_dictate::overlay::Style;
use parakeet_dictate::reducer::{DEFAULT_TAP_THRESHOLD_S, Event, IDLE, Transition, step};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GetDC, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongW, IsWindowVisible, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmFlush() -> i32;
}

const NEVER_FOCUSED_CLICK_THROUGH: u32 = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
const STRAIGHT: [u8; 4] = [255, 128, 0, 128];
const SIDE: usize = 8;

/// The tests that open a window run one at a time: dropping the last window of the shared class
/// unregisters the class, which would fail another test's window that is opening at that moment.
static WINDOWS: Mutex<()> = Mutex::new(());

fn one_window_test() -> MutexGuard<'static, ()> {
    WINDOWS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn test_premultiplied_bgra_on_a_known_pixel() {
    let mut bgra = [0; 4];
    premultiplied_bgra(&STRAIGHT, &mut bgra);
    // 255 * 128 / 255 = 128 and 128 * 128 / 255 = 64.25, in B, G, R, A order.
    assert_eq!(bgra, [0, 64, 128, 128]);
}

#[test]
fn test_premultiplied_bgra_never_exceeds_alpha() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut rgba: Vec<u8> = (0..64 * 64 * 4)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 56) as u8
        })
        .collect();
    // So the opaque case below always has pixels to look at.
    rgba.iter_mut().skip(3).step_by(4 * 16).for_each(|alpha| *alpha = 255);
    let mut bgra = vec![0; rgba.len()];
    premultiplied_bgra(&rgba, &mut bgra);
    for (straight, pixel) in rgba.chunks_exact(4).zip(bgra.chunks_exact(4)) {
        assert!(pixel[..3].iter().all(|channel| *channel <= pixel[3]), "{straight:?} became {pixel:?}");
        if straight[3] == 255 {
            assert_eq!(pixel[..3], [straight[2], straight[1], straight[0]]);
        }
    }
}

/// The screen's top-left SIDE by SIDE pixels as RGB, layered windows included.
fn grab_screen() -> Vec<[u8; 3]> {
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let memory = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIDE as i32,
            biHeight: -(SIDE as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(memory, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        assert!(!bitmap.is_null(), "CreateDIBSection failed");
        let previous = SelectObject(memory, bitmap);
        let copied = BitBlt(memory, 0, 0, SIDE as i32, SIDE as i32, screen, 0, 0, SRCCOPY | CAPTUREBLT);
        assert_ne!(copied, 0, "BitBlt failed");
        let bgra = std::slice::from_raw_parts(bits.cast::<u8>(), SIDE * SIDE * 4);
        let pixels = bgra.chunks_exact(4).map(|pixel| [pixel[2], pixel[1], pixel[0]]).collect();
        SelectObject(memory, previous);
        DeleteObject(bitmap);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        pixels
    }
}

/// `source` over an opaque `backdrop`, as `Image.alpha_composite` does it.
fn composite(source: [u8; 4], backdrop: [u8; 3]) -> [u8; 3] {
    let alpha = f64::from(source[3]) / 255.0;
    std::array::from_fn(|i| (f64::from(source[i]) * alpha + f64::from(backdrop[i]) * (1.0 - alpha)).round() as u8)
}

fn filled(color: [u8; 4]) -> Vec<u8> {
    color.repeat(SIDE * SIDE)
}

#[test]
fn test_update_layered_window_composites_the_pixel_as_straight_alpha_over_the_desktop() {
    let _one_at_a_time = one_window_test();
    // Physical pixels, like the overlay's own thread, so the grab reads what was drawn at any display scale.
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // An opaque window of a known colour sits under the test window, so a video or game
    // repainting the real desktop between the two grabs cannot change the expected value.
    let backdrop_color = [40, 90, 160, 255];
    let placement = Placement { x: 0, y: 0, width: SIDE as i32, height: SIDE as i32, scale: 1.0 };
    let (mut backdrop, mut window) = (Window::new().unwrap(), Window::new().unwrap());
    let style = unsafe { GetWindowLongW(window.hwnd(), GWL_EXSTYLE) } as u32;
    assert_eq!(style & NEVER_FOCUSED_CLICK_THROUGH, NEVER_FOCUSED_CLICK_THROUGH);
    backdrop.draw(&filled(backdrop_color), placement).unwrap();
    window.draw(&filled(STRAIGHT), placement).unwrap();
    unsafe { DwmFlush() };
    std::thread::sleep(Duration::from_millis(50));
    let shown = grab_screen();
    window.hide();
    backdrop.hide();

    let expected = composite(STRAIGHT, [backdrop_color[0], backdrop_color[1], backdrop_color[2]]);
    for pixel in shown {
        let difference = pixel.iter().zip(expected).map(|(a, b)| a.abs_diff(b)).max().unwrap();
        assert!(difference <= 2, "desktop {pixel:?} vs straight-alpha composite {expected:?}");
    }
}

fn wait_for(probe: impl Fn() -> bool, what: &str, timeout_s: f64) {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout_s);
    while !probe() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn start_recording(overlay: &Overlay) {
    let event = Event::HotkeyDown(clock::now());
    let (state, effects) = step(&IDLE, &event, DEFAULT_TAP_THRESHOLD_S, false);
    overlay.notify(&Transition { previous: IDLE, state, event, effects });
}

fn frames_by(overlay: &Overlay, style: Style) -> u64 {
    overlay.stats.style_frames[style as usize].load(Ordering::Relaxed)
}

#[test]
fn test_restyle_swaps_the_renderer_mid_dictation_and_off_hides_the_window_until_restyled() {
    let _one_at_a_time = one_window_test();
    let (first, second) = (Style::Fluid, Style::Bars);
    let meter = LevelMeter::new(16_000);
    let mut overlay = Overlay::new(first, move || meter.snapshot()).unwrap();
    let hwnd = overlay.stats.hwnd.load(Ordering::Acquire) as HWND;
    let visible = || unsafe { IsWindowVisible(hwnd) } != 0;

    start_recording(&overlay);
    wait_for(visible, "the recording to show", 2.0);
    overlay.restyle(Some(second));
    wait_for(|| frames_by(&overlay, second) > 0, "the second renderer to draw", 2.0);
    let drawn_by_first = frames_by(&overlay, first);
    overlay.restyle(None);
    wait_for(|| !visible(), "off to hide the window", 3.0 / FPS);
    assert_eq!(frames_by(&overlay, first), drawn_by_first);

    start_recording(&overlay);
    std::thread::sleep(Duration::from_secs_f64(SHOW_DELAY_S + 0.1));
    assert!(!visible(), "a transition while off showed the window");

    overlay.restyle(Some(first));
    start_recording(&overlay);
    wait_for(visible, "the window to show again once a renderer is set", 2.0);
    assert!(frames_by(&overlay, first) > drawn_by_first);
    overlay.close();
}
