//! Developer checks, run by hand with the tray app stopped.
//!
//! e2e           Real keyboard hook, overlay window, clipboard and paste into a target window. Post-processing
//!               talks to a local fake, never a provider. Do not touch the keyboard or mouse while it runs.
//! hook-latency  How long a key takes to reach the hook, with the overlay hidden and animating.
//! mic           Two seconds from the default microphone: device, level, open time, meter.
//! model         Transcribe the fixtures and report accuracy, latency, and the engine that ran.
//! overlay       The real overlay through scripted dictations: handles, CPU, frame rate, render time. No keys, no hook.
//! polish        Sample dictations through the selected post-processor, which spends the provider's key.
//! server        The server for Mac on 127.0.0.1 with the real model; the fixtures go through it over HTTP.
//! target        The window e2e types into; e2e starts it.
//!
//! polish and server take --fake-provider: a local fake replaces the provider, with no key and no network.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use parakeet_dictate::clock;
use parakeet_dictate::config::{Config, Engine, PolishProvider, PostprocessConfig, ServerConfig, load_api_key, load_config};
use parakeet_dictate::cues::PlayCue;
use parakeet_dictate::dictation::{Dictation, DictationParts};
use parakeet_dictate::inject::{self, Stroke};
use parakeet_dictate::instance::{MUTEX_NAME, acquire_named};
use parakeet_dictate::keyboard::KeyboardHook;
use parakeet_dictate::meter::LevelMeter;
use parakeet_dictate::overlay::Style;
use parakeet_dictate::overlay::active::{ActiveOverlay, OpenWindow};
use parakeet_dictate::overlay::frame::{CANVAS_SIZE, FPS, OverlayFrame, Phase};
use parakeet_dictate::overlay::scene::SHOW_DELAY_S;
use parakeet_dictate::overlay::window::{Overlay, OverlaySurface, OverlayStats};
use parakeet_dictate::paths;
use parakeet_dictate::polish::{Headers, Polish, PolishError, Polisher, Reply, Transport, TransportError, create_polisher};
use parakeet_dictate::recorder::{Audio, Microphone, Record};
use parakeet_dictate::reducer::{Cue, DEFAULT_TAP_THRESHOLD_S, Event, IDLE, State, Transition, step};
use parakeet_dictate::server::{DictationServer, HEALTH_PATH, ServerParts, TRANSCRIPTIONS_PATH, parse_wav};
use parakeet_dictate::text::{is_word_char, short_float};
use parakeet_dictate::transcriber::{SharedModel, Transcribe, Transcriber, resample};
use parakeet_dictate::win::{from_wide, wide};
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::{FILETIME, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, TRUE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetCurrentThreadId, GetGuiResources, GetThreadTimes, OpenThread,
    THREAD_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Controls::EM_SETSEL;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DispatchMessageW, ES_AUTOVSCROLL, ES_MULTILINE, GWL_EXSTYLE, GetClientRect,
    GetForegroundWindow, GetMessageW, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, MSG, MoveWindow, PostMessageW, PostQuitMessage, RegisterClassExW, SW_RESTORE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW,
    SetForegroundWindow, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, WM_APP, WM_DESTROY, WM_SETFOCUS, WM_SIZE, WNDCLASSEXW,
    WS_CHILD, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WindowFromPoint,
};

const MIC_RATE: u32 = 44_100;
const SENTINEL: &str = "parakeet-e2e clipboard sentinel ✓";
const MAX_WER: f64 = 0.15;
const HOTKEY: u16 = 0xA3;
const VK_C: u16 = 0x43;
const VK_F24: u16 = 0x87;
const VK_ESCAPE: u16 = inject::VK_ESCAPE as u16;
const UNREACHABLE_ENDPOINT: &str = "https://127.0.0.1:9/api/coding/paas/v4/chat/completions";
const TINY_TIMEOUT_S: f64 = 0.3;
const WAIT_S: f64 = 15.0;
const POLISH_DELAY: Duration = Duration::from_millis(1500);
const FEED_CHUNK_S: f64 = 0.02;
const WATCH_INTERVAL: Duration = Duration::from_millis(5);
// The shortcut flow's second key comes well inside the overlay's show delay.
const SHORTCUT_GAP_S: f64 = 0.6 * SHOW_DELAY_S;
const OVERLAY_EXSTYLE: u32 = 0x0008_0000 | 0x0000_0020 | 0x0800_0000 | 0x0000_0080 | 0x0000_0008;
const BOTTOM_MARGIN: f64 = 48.0;
const TARGET_CLASS: &str = "ParakeetDictateE2eTarget";
const MIC_SECONDS: f64 = 2.0;
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const BOUNDARY: &str = "parakeet-check-boundary";
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
const CYCLES: usize = 10;
const LONG_FRAMES: u64 = 300;
const LONG_RECORDING_LIMIT_S: f64 = 30.0;
const IDLE_S: f64 = 10.0;

// The windows-sys feature that binds it, Win32_System_WindowsProgramming, is off for the app.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryThreadCycleTime(thread: HANDLE, cycles: *mut u64) -> i32;
}

fn sleep(seconds: f64) {
    thread::sleep(Duration::from_secs_f64(seconds));
}

fn fixtures_folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn fixture(name: &str) -> (Audio, String) {
    let folder = fixtures_folder();
    let data = fs::read(folder.join(format!("{name}.wav"))).expect("the fixture exists");
    let audio = parse_wav(&data).expect("the fixture is a WAV file");
    let expected = fs::read_to_string(folder.join(format!("{name}.txt"))).expect("the fixture's text exists");
    (audio, expected.trim().to_owned())
}

fn at_mic_rate((audio, expected): (Audio, String)) -> (Audio, String) {
    (Audio { samples: resample(&audio.samples, audio.rate, MIC_RATE), rate: MIC_RATE }, expected)
}

fn words(text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    lowered.split(|c: char| !is_word_char(c)).filter(|word| !word.is_empty()).map(str::to_owned).collect()
}

fn word_error_rate(reference: &[String], heard: &[String]) -> f64 {
    let mut previous: Vec<usize> = (0..=heard.len()).collect();
    for (i, word) in reference.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, other) in heard.iter().enumerate() {
            current.push((previous[j + 1] + 1).min(current[j] + 1).min(previous[j] + usize::from(word != other)));
        }
        previous = current;
    }
    previous[heard.len()] as f64 / reference.len().max(1) as f64
}

fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let position = (sorted.len() - 1) as f64 * q;
    let low = position as usize;
    let high = (low + 1).min(sorted.len() - 1);
    sorted[low] + (sorted[high] - sorted[low]) * (position - low as f64)
}

struct Silent;

impl PlayCue for Silent {
    fn play(&self, _: Cue) {}
}

fn option(arguments: &[String], name: &str) -> Option<String> {
    arguments.iter().position(|argument| argument == name).and_then(|at| arguments.get(at + 1)).cloned()
}

fn style_argument(arguments: &[String]) -> Style {
    option(arguments, "--style").and_then(|name| Style::named(&name)).unwrap_or(Style::Fluid)
}

fn acquire_instance() -> Result<parakeet_dictate::instance::Handle, i32> {
    match acquire_named(MUTEX_NAME) {
        Ok(Some(handle)) => Ok(handle),
        _ => {
            println!("parakeet-dictate is running; stop it first (parakeet-dictate --quit)");
            Err(2)
        }
    }
}

// --- the target window ----------------------------------------------------------------------------------

static TARGET_EDIT: AtomicUsize = AtomicUsize::new(0);
static TARGET_WINDOW: AtomicUsize = AtomicUsize::new(0);
static TARGET_COMMANDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn edit() -> HWND {
    TARGET_EDIT.load(Ordering::Acquire) as HWND
}

fn reply(message: &Value) {
    println!("{message}");
    let _ = std::io::stdout().flush();
}

fn target_placement(hwnd: HWND) -> Value {
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetWindowRect(hwnd, &raw mut rect) };
    json!({"ok": true, "origin": [rect.left, rect.top], "center": [(rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2]})
}

fn bring_to_front(hwnd: HWND) {
    // Windows refuses SetForegroundWindow from a background process unless its input is
    // attached to the current foreground thread.
    unsafe {
        let foreground = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
        let me = GetCurrentThreadId();
        let attached = foreground != 0 && foreground != me && AttachThreadInput(me, foreground, TRUE) != 0;
        ShowWindow(hwnd, SW_RESTORE);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        if attached {
            AttachThreadInput(me, foreground, 0);
        }
        SetFocus(edit());
    }
}

fn target_text() -> String {
    let control = edit();
    let length = unsafe { GetWindowTextLengthW(control) };
    let mut buffer = vec![0u16; length as usize + 1];
    unsafe { GetWindowTextW(control, buffer.as_mut_ptr(), buffer.len() as i32) };
    from_wide(&buffer)
}

fn run_target_command(hwnd: HWND, command: &str) {
    let parts: Vec<&str> = command.split_whitespace().collect();
    match parts.as_slice() {
        ["focus"] => {
            bring_to_front(hwnd);
            reply(&target_placement(hwnd));
        }
        ["move", x, y] => {
            let (x, y) = (x.parse().unwrap_or(0), y.parse().unwrap_or(0));
            unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER) };
            reply(&target_placement(hwnd));
        }
        ["text"] => reply(&json!({"text": target_text()})),
        ["clear"] => {
            unsafe {
                SetWindowTextW(edit(), wide("").as_ptr());
                SendMessageW(edit(), EM_SETSEL, 0, 0);
            }
            reply(&json!({"ok": true}));
        }
        ["quit"] => unsafe {
            PostQuitMessage(0);
        },
        _ => {}
    }
}

unsafe extern "system" fn target_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_APP => {
            let commands = std::mem::take(&mut *TARGET_COMMANDS.lock().unwrap());
            for command in commands {
                run_target_command(hwnd, &command);
            }
            0
        }
        WM_SIZE => {
            let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe {
                GetClientRect(hwnd, &raw mut client);
                MoveWindow(edit(), 0, 0, client.right, client.bottom, TRUE);
            }
            0
        }
        WM_SETFOCUS => {
            unsafe { SetFocus(edit()) };
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn run_target() -> i32 {
    let class = wide(TARGET_CLASS);
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let mut description: WNDCLASSEXW = std::mem::zeroed();
        description.cbSize = size_of::<WNDCLASSEXW>() as u32;
        description.lpfnWndProc = Some(target_proc);
        description.hInstance = instance;
        description.lpszClassName = class.as_ptr();
        RegisterClassExW(&raw const description);
        let title = wide("parakeet-dictate e2e target");
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            200,
            200,
            640,
            240,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        let control = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | (ES_MULTILINE | ES_AUTOVSCROLL) as u32,
            0,
            0,
            640,
            240,
            hwnd,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        TARGET_EDIT.store(control as usize, Ordering::Release);
        TARGET_WINDOW.store(hwnd as usize, Ordering::Release);
        thread::spawn(|| {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else {
                    break;
                };
                TARGET_COMMANDS.lock().unwrap().push(line);
                PostMessageW(TARGET_WINDOW.load(Ordering::Acquire) as HWND, WM_APP, 0, 0);
            }
            TARGET_COMMANDS.lock().unwrap().push("quit".to_owned());
            PostMessageW(TARGET_WINDOW.load(Ordering::Acquire) as HWND, WM_APP, 0, 0);
        });
        reply(&json!({"pid": std::process::id()}));
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
    0
}

struct Target {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    pid: u32,
}

impl Target {
    fn open() -> Self {
        let exe = std::env::current_exe().expect("this program");
        let mut child = Process::new(exe).arg("target").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("the target starts");
        let input = child.stdin.take().expect("a pipe");
        let mut output = BufReader::new(child.stdout.take().expect("a pipe"));
        let mut line = String::new();
        output.read_line(&mut line).expect("the target says hello");
        let pid = serde_json::from_str::<Value>(&line).expect("a JSON greeting")["pid"].as_u64().expect("a pid") as u32;
        Self { child, input, output, pid }
    }

    fn ask(&mut self, command: &str) -> Value {
        writeln!(self.input, "{command}").expect("the target reads");
        self.input.flush().expect("the target reads");
        let mut line = String::new();
        self.output.read_line(&mut line).expect("the target replies");
        serde_json::from_str(&line).expect("a JSON reply")
    }

    fn text(&mut self) -> String {
        self.ask("text")["text"].as_str().unwrap_or_default().to_owned()
    }

    fn close(mut self) {
        let _ = writeln!(self.input, "quit");
        let _ = self.input.flush();
        let _ = self.child.wait();
    }
}

fn foreground_pid() -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &raw mut pid) };
    pid
}

/// Keys pressed like a person's: untagged, and only into the target window.
struct Keyboard {
    pid: u32,
    held: Mutex<BTreeSet<u16>>,
}

impl Keyboard {
    fn require_foreground(&self) -> Result<(), String> {
        let pid = foreground_pid();
        if pid == self.pid { Ok(()) } else { Err(format!("foreground is pid {pid}, not the target window (pid {})", self.pid)) }
    }

    fn press(&self, vk: u16) -> Result<f64, String> {
        self.require_foreground()?;
        let at = clock::now();
        inject::send_keys(&[Stroke { vk, up: false }], 0)?;
        self.held.lock().unwrap().insert(vk);
        Ok(at)
    }

    fn release(&self, vk: u16) -> Result<f64, String> {
        self.require_foreground()?;
        let at = clock::now();
        inject::send_keys(&[Stroke { vk, up: true }], 0)?;
        self.held.lock().unwrap().remove(&vk);
        Ok(at)
    }

    fn auto_repeat(&self, vk: u16, times: usize, interval_s: f64) -> Result<(), String> {
        for _ in 0..times {
            sleep(interval_s);
            self.press(vk)?;
        }
        Ok(())
    }

    fn release_held_keys_unguarded(&self) {
        let held = std::mem::take(&mut *self.held.lock().unwrap());
        for vk in held {
            let _ = inject::send_keys(&[Stroke { vk, up: true }], 0);
        }
    }
}

/// Plays the fixture into the level meter in real time, as the microphone would, and hands the whole
/// fixture to the transcriber on stop. Clones share one recording.
#[derive(Clone)]
struct FixtureRecorder {
    audio: Arc<Mutex<Option<Audio>>>,
    meter: Arc<Mutex<Arc<LevelMeter>>>,
    discards: Arc<AtomicUsize>,
    halt: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    feeder: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl FixtureRecorder {
    fn new() -> Self {
        Self {
            audio: Arc::default(),
            meter: Arc::new(Mutex::new(Arc::new(LevelMeter::new(MIC_RATE)))),
            discards: Arc::default(),
            halt: Arc::default(),
            feeder: Arc::default(),
        }
    }

    fn levels(&self) -> impl Fn() -> (parakeet_dictate::meter::Levels, parakeet_dictate::meter::Spectrum) + Send + 'static {
        let meter = self.meter.clone();
        move || meter.lock().unwrap().clone().snapshot()
    }

    fn stop_feeding(&self) {
        if let Some(halt) = self.halt.lock().unwrap().take() {
            halt.store(true, Ordering::Release);
        }
        if let Some(feeder) = self.feeder.lock().unwrap().take() {
            let _ = feeder.join();
        }
    }
}

impl Record for FixtureRecorder {
    fn start(&mut self) -> Result<(), String> {
        self.stop_feeding();
        let audio = self.audio.lock().unwrap().clone().ok_or("no fixture")?;
        let meter = Arc::new(LevelMeter::new(audio.rate));
        *self.meter.lock().unwrap() = meter.clone();
        let halt = Arc::new(AtomicBool::new(false));
        *self.halt.lock().unwrap() = Some(halt.clone());
        let feeder = thread::spawn(move || {
            let chunk = (f64::from(audio.rate) * FEED_CHUNK_S).round() as usize;
            let started = clock::now();
            for (index, samples) in audio.samples.chunks(chunk).enumerate() {
                let due = started + (index + 1) as f64 * FEED_CHUNK_S;
                let wait = (due - clock::now()).max(0.0);
                if halt.load(Ordering::Acquire) {
                    return;
                }
                sleep(wait);
                meter.feed(samples);
            }
        });
        *self.feeder.lock().unwrap() = Some(feeder);
        Ok(())
    }

    fn stop(&mut self) -> Result<Audio, String> {
        self.stop_feeding();
        self.audio.lock().unwrap().clone().ok_or_else(|| "no fixture".to_owned())
    }

    fn discard(&mut self) {
        self.stop_feeding();
        self.discards.fetch_add(1, Ordering::AcqRel);
    }

    fn device_name(&self) -> String {
        "fixture".to_owned()
    }
}

/// Answers like a provider after `delay`, with the transcript unchanged, so accuracy still reads the transcription.
struct FakeProvider {
    delay: Duration,
}

impl Transport for FakeProvider {
    fn options(&self, _: &str, _: &Headers) -> Result<Reply, TransportError> {
        Ok(Reply { status: 204, body: Vec::new() })
    }

    fn post(&self, _: &str, _: &Headers, body: &[u8]) -> Result<Reply, TransportError> {
        thread::sleep(self.delay);
        let body: Value = serde_json::from_slice(body).map_err(|e| TransportError::Failed(e.to_string()))?;
        let user = body["messages"][1]["content"].as_str().unwrap_or("");
        let raw = user.trim_start_matches("<transcript>").trim_end_matches("</transcript>");
        let reply = json!({
            "choices": [{"message": {"content": raw}, "finish_reason": "stop"}],
            "usage": {"completion_tokens_details": {"reasoning_tokens": 0}},
        });
        Ok(Reply { status: 200, body: serde_json::to_vec(&reply).expect("a reply serializes") })
    }
}

/// A client that records each finished request, so a flow can wait for a reply it expects to be ignored.
struct Counting {
    inner: Arc<dyn Polish>,
    finished: Mutex<Vec<String>>,
}

impl Polish for Counting {
    fn preconnect(&self) {
        self.inner.preconnect();
    }

    fn polish(&self, raw: &str, instructions: &str) -> Result<String, PolishError> {
        let result = self.inner.polish(raw, instructions);
        self.finished.lock().unwrap().push(result.as_ref().map_or_else(|e| format!("failed: {e}"), |_| "polished".to_owned()));
        result
    }
}

impl Counting {
    fn count(&self) -> usize {
        self.finished.lock().unwrap().len()
    }

    fn last(&self) -> String {
        self.finished.lock().unwrap().last().cloned().unwrap_or_default()
    }

    fn wait_for_reply(&self, count: usize) -> Result<String, String> {
        let deadline = clock::now() + WAIT_S;
        while self.count() < count {
            if clock::now() > deadline {
                return Err(format!("request {count} did not finish within {WAIT_S} s"));
            }
            sleep(0.02);
        }
        Ok(self.finished.lock().unwrap()[count - 1].clone())
    }
}

#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[derive(Default)]
struct Watched {
    samples: u64,
    gaps: Vec<f64>,
    focused: u64,
    hit_tests: u64,
    hits: u64,
    rect: Rect,
    shown: Vec<(f64, Option<f64>)>,
    phases: Vec<(f64, u32)>,
    recording_level_max: f32,
}

/// Samples the overlay on its own thread, in physical pixels: who has focus, whether the overlay is
/// visible, what is under its center, and what it draws.
struct OverlayWatch {
    seen: Arc<Mutex<Watched>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn phase_code(phase: Phase) -> u32 {
    phase as u32 + 1
}

impl OverlayWatch {
    fn start(stats: Arc<OverlayStats>, levels: impl Fn() -> (parakeet_dictate::meter::Levels, parakeet_dictate::meter::Spectrum) + Send + 'static) -> Self {
        let seen = Arc::new(Mutex::new(Watched::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen_now, stopping) = (seen.clone(), stop.clone());
        let thread = thread::spawn(move || {
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
            let hwnd = stats.hwnd.load(Ordering::Acquire) as HWND;
            let mut last = clock::now();
            while !stopping.load(Ordering::Acquire) {
                thread::sleep(WATCH_INTERVAL);
                let now = clock::now();
                let mut seen = seen_now.lock().unwrap();
                seen.gaps.push(now - last);
                last = now;
                seen.samples += 1;
                unsafe {
                    if GetForegroundWindow() == hwnd {
                        seen.focused += 1;
                    }
                    let visible = IsWindowVisible(hwnd) != 0;
                    let open = seen.shown.last().is_some_and(|(_, hidden)| hidden.is_none());
                    if visible && !open {
                        seen.shown.push((now, None));
                    } else if !visible && open {
                        seen.shown.last_mut().expect("open means present").1 = Some(now);
                    }
                    if visible {
                        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                        GetWindowRect(hwnd, &raw mut rect);
                        seen.rect = Rect { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom };
                        seen.hit_tests += 1;
                        let center = POINT { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 };
                        if WindowFromPoint(center) == hwnd {
                            seen.hits += 1;
                        }
                    }
                }
                let phase = stats.phase.load(Ordering::Acquire);
                if seen.phases.last().is_none_or(|(_, last)| *last != phase) {
                    seen.phases.push((now, phase));
                }
                if phase == phase_code(Phase::Recording) {
                    let most = levels().0.iter().copied().fold(0.0f32, f32::max);
                    seen.recording_level_max = seen.recording_level_max.max(most);
                }
            }
        });
        Self { seen, stop, thread: Some(thread) }
    }

    fn visible_between(&self, start: f64, end: f64) -> bool {
        self.seen.lock().unwrap().shown.iter().any(|(shown, hidden)| *shown <= end && hidden.is_none_or(|hidden| hidden >= start))
    }

    fn hidden_after(&self, t: f64) -> Option<f64> {
        let seen = self.seen.lock().unwrap();
        seen.shown.iter().find_map(|(shown, hidden)| hidden.filter(|hidden| *shown <= t && *hidden >= t))
    }

    fn shown_since(&self, t: f64) -> bool {
        self.seen.lock().unwrap().shown.iter().any(|(shown, _)| *shown > t)
    }

    fn phases_between(&self, start: f64, end: f64) -> Vec<u32> {
        let seen = self.seen.lock().unwrap();
        seen.phases.iter().filter(|(t, phase)| start <= *t && *t <= end && *phase != 0).map(|(_, phase)| *phase).collect()
    }

    fn rect(&self) -> Rect {
        self.seen.lock().unwrap().rect
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Monitors(Vec<(Rect, u32)>);

unsafe extern "system" fn visit_monitor(monitor: HMONITOR, _: HDC, _: *mut RECT, found: LPARAM) -> i32 {
    unsafe {
        let found = &mut *(found as *mut Monitors);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(monitor, &raw mut info);
        let (mut dpi, mut other) = (96, 96);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi, &raw mut other);
        let work = info.rcWork;
        found.0.push((Rect { left: work.left, top: work.top, right: work.right, bottom: work.bottom }, dpi));
    }
    TRUE
}

/// Each monitor's work area and DPI, in physical pixels.
fn work_areas() -> Vec<(Rect, u32)> {
    let mut found = Monitors(Vec::new());
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(visit_monitor), (&raw mut found) as LPARAM);
        SetThreadDpiAwarenessContext(previous);
    }
    found.0
}

fn wait_until<T>(mut probe: impl FnMut() -> Option<T>, what: &str, timeout_s: f64) -> Result<(T, f64), String> {
    let deadline = clock::now() + timeout_s;
    while clock::now() < deadline {
        if let Some(found) = probe() {
            return Ok((found, clock::now()));
        }
        sleep(0.005);
    }
    Err(format!("timed out waiting for {what}"))
}

struct E2e {
    target: Target,
    keyboard: Arc<Keyboard>,
    recorder: FixtureRecorder,
    en: (Audio, String),
    pt: (Audio, String),
    model: Arc<SharedModel>,
    overlay: Arc<Mutex<Overlay>>,
    stats: Arc<OverlayStats>,
    watch: OverlayWatch,
    states: Arc<Mutex<Vec<(f64, State)>>>,
    pastes: Arc<Mutex<Vec<f64>>>,
    latencies: Vec<(String, f64)>,
    failures: Vec<String>,
    escapes_leaked: Arc<AtomicU32>,
    aborted: Arc<Mutex<Option<String>>>,
}

impl E2e {
    fn check(&mut self, ok: bool, message: &str) {
        println!("  {} {message}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.failures.push(message.to_owned());
        }
    }

    fn fixture(&self, name: &str) -> &(Audio, String) {
        if name == "en" { &self.en } else { &self.pt }
    }

    fn dictating(&self, polisher: Option<Arc<dyn Polish>>, enabled: bool, config: Config) -> Result<Dictation, String> {
        let overlay = self.overlay.clone();
        let states = self.states.clone();
        let on_transition = Box::new(move |transition: &Transition| {
            states.lock().unwrap().push((clock::now(), transition.state.clone()));
            overlay.lock().unwrap().notify(transition);
        });
        let mut parts = DictationParts::new(
            config,
            Box::new(self.recorder.clone()),
            self.model.clone() as Arc<dyn Transcribe>,
            Arc::new(Silent),
            on_transition,
            polisher,
            Arc::new(move || enabled),
        );
        let (keyboard, pastes, aborted) = (self.keyboard.clone(), self.pastes.clone(), self.aborted.clone());
        parts.paste = Box::new(move |text| {
            if let Err(e) = keyboard.require_foreground() {
                *aborted.lock().unwrap() = Some(e.clone());
                return Err(e);
            }
            pastes.lock().unwrap().push(clock::now());
            inject::paste(text)
        });
        let dictation = Dictation::new(parts);
        dictation.listen()?;
        Ok(dictation)
    }

    fn polishing_after(&self, t: f64) -> Option<(f64, String)> {
        let states = self.states.lock().unwrap();
        states.iter().find_map(|(at, state)| match state {
            State::Polishing { raw, .. } if *at >= t => Some((*at, raw.clone())),
            _ => None,
        })
    }

    fn deferred(&self, pressed: f64, released: f64) -> bool {
        let states = self.states.lock().unwrap();
        states.iter().any(|(t, state)| matches!(state, State::AwaitingRelease { .. }) && pressed < *t && *t < released)
    }

    fn prepare(&mut self, name: &str) -> Result<(), String> {
        self.target.ask("clear");
        inject::set_text(Some(SENTINEL), true)?;
        *self.recorder.audio.lock().unwrap() = Some(self.fixture(name).0.clone());
        Ok(())
    }

    fn hold(&mut self, name: &str, watch_recording: bool) -> Result<(f64, Rect), String> {
        self.prepare(name)?;
        self.keyboard.press(HOTKEY)?;
        self.keyboard.auto_repeat(HOTKEY, 4, 0.2)?;
        let mut rect = Rect::default();
        if watch_recording {
            let phase = self.stats.phase.load(Ordering::Acquire);
            let hwnd = self.stats.hwnd.load(Ordering::Acquire) as HWND;
            let showing = phase == phase_code(Phase::Recording) && unsafe { IsWindowVisible(hwnd) } != 0;
            self.check(showing, "overlay visible mid-recording, drawing RECORDING");
            rect = self.watch.rect();
        }
        sleep(0.2);
        Ok((self.keyboard.release(HOTKEY)?, rect))
    }

    fn check_bottom_center(&mut self, rect: Rect, work: Rect, dpi: u32, place: &str) {
        let center_off = f64::from(rect.left + rect.right) / 2.0 - f64::from(work.left + work.right) / 2.0;
        let margin = work.bottom - rect.bottom;
        let expected = BOTTOM_MARGIN * f64::from(dpi) / 96.0;
        let inside = work.left <= rect.left && rect.right <= work.right;
        let message = format!(
            "overlay at the bottom center of the {place} work area ({},{} to {},{}; work area {},{} to {},{}; {margin} px above its bottom)",
            rect.left, rect.top, rect.right, rect.bottom, work.left, work.top, work.right, work.bottom,
        );
        self.check(center_off.abs() <= 1.0 && (f64::from(margin) - expected).abs() <= 1.0 && inside, &message);
    }

    fn landed(&mut self, name: &str, flow: &str, released: f64) -> Result<String, String> {
        let target = &mut self.target;
        let (text, seen_at) = wait_until(
            || {
                let text = target.text();
                (!text.is_empty()).then_some(text)
            },
            "text in the target window",
            WAIT_S,
        )?;
        sleep(inject::CLIPBOARD_RESTORE_DELAY.as_secs_f64() + 0.3);
        let wer = word_error_rate(&words(&self.fixture(name).1), &words(&text));
        let last_paste = *self.pastes.lock().unwrap().last().ok_or("no paste happened")?;
        let paste_ms = (last_paste - released) * 1000.0;
        self.latencies.push((format!("{flow} {name}"), paste_ms));
        // The text is the fixture's public sample, never a real dictation.
        println!("[{flow} {name}] got: {text:?}");
        println!("  key-up to Ctrl+V sent {paste_ms:.0} ms, to text visible in target {:.0} ms", (seen_at - released) * 1000.0);
        self.check(wer <= MAX_WER, &format!("WER {wer:.2} <= {MAX_WER}"));
        self.check(text.ends_with(' ') && !text.ends_with("  "), "one trailing space appended");
        let clipboard = inject::get_text()?;
        self.check(clipboard.as_deref() == Some(SENTINEL), "clipboard restored to the sentinel");
        Ok(text)
    }

    fn work_area_at(x: i32, y: i32) -> Option<(Rect, u32)> {
        work_areas().into_iter().find(|(work, _)| work.left <= x && x < work.right && work.top <= y && y < work.bottom)
    }

    fn finish(&self, dictation: Dictation) {
        self.keyboard.release_held_keys_unguarded();
        dictation.stop();
    }

    fn flows_without_polish(&mut self, home: &Value, config: &Config) -> Result<(), String> {
        let dictation = self.dictating(None, true, config.clone())?;
        for name in ["en", "pt"] {
            let (released, rect) = self.hold(name, true)?;
            self.landed(name, "hold", released)?;
            let center = (home["center"][0].as_i64().unwrap_or(0) as i32, home["center"][1].as_i64().unwrap_or(0) as i32);
            let (work, dpi) = Self::work_area_at(center.0, center.1).ok_or("the target is on no monitor")?;
            self.check_bottom_center(rect, work, dpi, "target's");
        }

        for name in ["en", "pt"] {
            self.prepare(name)?;
            self.keyboard.press(HOTKEY)?;
            sleep(0.08);
            self.keyboard.release(HOTKEY)?;
            sleep(1.0);
            let empty = self.target.text().is_empty();
            self.check(empty, "tap left recording on, nothing pasted yet");
            let pressed = self.keyboard.press(HOTKEY)?;
            sleep(0.8);
            let empty = self.target.text().is_empty();
            self.check(empty, "nothing pasted while the hotkey is still down");
            let released = self.keyboard.release(HOTKEY)?;
            let deferred = self.deferred(pressed, released);
            self.check(deferred, "transcript was ready before key-up and waited for it");
            self.landed(name, "toggle", released)?;
        }

        self.prepare("en")?;
        let discards = self.recorder.discards.load(Ordering::Acquire);
        let leaked = self.escapes_leaked.load(Ordering::Acquire);
        self.keyboard.press(HOTKEY)?;
        sleep(0.6);
        self.keyboard.press(VK_ESCAPE)?;
        self.keyboard.release(VK_ESCAPE)?;
        sleep(0.2);
        self.keyboard.release(HOTKEY)?;
        sleep(1.5);
        self.keyboard.require_foreground()?;
        println!("[esc cancel while holding]");
        let empty = self.target.text().is_empty();
        self.check(empty, "Esc cancelled: nothing pasted");
        self.check(self.recorder.discards.load(Ordering::Acquire) > discards, "recording discarded");
        let clipboard = inject::get_text()?;
        self.check(clipboard.as_deref() == Some(SENTINEL), "clipboard untouched");
        self.check(self.escapes_leaked.load(Ordering::Acquire) == leaked, "Esc swallowed by the app, so Ctrl+Esc never reached Windows");

        println!("[shortcut: Right Ctrl, then C {:.0} ms later]", SHORTCUT_GAP_S * 1000.0);
        self.prepare("en")?;
        let pressed = self.keyboard.press(HOTKEY)?;
        sleep(SHORTCUT_GAP_S);
        self.keyboard.press(VK_C)?;
        self.keyboard.release(VK_C)?;
        self.keyboard.release(HOTKEY)?;
        sleep(1.0);
        let discarded = {
            let states = self.states.lock().unwrap();
            states.iter().any(|(t, state)| matches!(state, State::Idle { key_down: true }) && *t > pressed)
        };
        self.check(discarded, "the shortcut discarded the recording");
        self.check(!self.watch.visible_between(pressed, clock::now()), "overlay never became visible");
        let (empty, clipboard) = (self.target.text().is_empty(), inject::get_text()?);
        self.check(empty && clipboard.as_deref() == Some(SENTINEL), "nothing pasted, clipboard untouched");

        println!("[shortcut after the overlay appeared]");
        self.prepare("en")?;
        self.keyboard.press(HOTKEY)?;
        let hwnd = self.stats.hwnd.load(Ordering::Acquire) as HWND;
        wait_until(|| (unsafe { IsWindowVisible(hwnd) } != 0).then_some(()), "the overlay to appear", 2.0)?;
        sleep(0.2);
        let second_key = self.keyboard.press(VK_C)?;
        self.keyboard.release(VK_C)?;
        self.keyboard.release(HOTKEY)?;
        sleep(1.0);
        let hidden_at = self.watch.hidden_after(second_key);
        let hid_ms = hidden_at.map_or(f64::NAN, |hidden| (hidden - second_key) * 1000.0);
        self.check(hidden_at.is_some_and(|hidden| hidden - second_key < 0.05), &format!("overlay hid {hid_ms:.0} ms after the second key"));
        self.check(!self.watch.shown_since(second_key), "and did not come back");
        let beats = [Phase::Done, Phase::Cancelled, Phase::Failed].map(phase_code);
        let played = self.watch.phases_between(second_key, clock::now()).into_iter().filter(|phase| beats.contains(phase)).count();
        self.check(played == 0, &format!("no beat played ({played})"));

        let left = work_areas().into_iter().find(|(work, _)| work.right <= 0);
        match left {
            None => println!("[hold on a monitor left of the primary] skipped: no such monitor"),
            Some((work, dpi)) => {
                self.target.ask(&format!("move {} {}", work.left + 400, work.top + 200));
                let moved = self.target.ask("focus");
                sleep(0.3);
                let center = (moved["center"][0].as_i64().unwrap_or(0) as i32, moved["center"][1].as_i64().unwrap_or(0) as i32);
                let there = Self::work_area_at(center.0, center.1).is_some_and(|(area, _)| area.left == work.left);
                self.check(there, &format!("target moved to the left monitor (center {center:?})"));
                let (released, rect) = self.hold("en", true)?;
                self.landed("en", "hold on the left monitor", released)?;
                self.check_bottom_center(rect, work, dpi, "left monitor's");
                self.target.ask(&format!("move {} {}", home["origin"][0], home["origin"][1]));
                self.target.ask("focus");
                sleep(0.3);
                self.keyboard.require_foreground()?;
            }
        }
        self.finish(dictation);
        Ok(())
    }

    fn flows_with_polish(&mut self, config: &Config) -> Result<(), String> {
        let transport = Box::new(FakeProvider { delay: POLISH_DELAY });
        let inner: Arc<dyn Polish> = Arc::new(create_polisher(&config.postprocess, "fake", Some(transport)));
        let provider = Arc::new(Counting { inner, finished: Mutex::new(Vec::new()) });

        let dictation = self.dictating(Some(provider.clone()), false, config.clone())?;
        let requests = provider.count();
        let (released, _) = self.hold("en", false)?;
        self.landed("en", "tray toggle off", released)?;
        let quiet = self.polishing_after(released).is_none() && provider.count() == requests;
        self.check(quiet, "provider not called");
        self.finish(dictation);

        let dictation = self.dictating(Some(provider.clone()), true, config.clone())?;
        for name in ["en", "pt"] {
            let (released, _) = self.hold(name, false)?;
            self.landed(name, "polished hold", released)?;
            let polished = self.polishing_after(released).is_some() && provider.last() == "polished";
            self.check(polished, "went through the provider");
        }

        self.prepare("pt")?;
        self.keyboard.press(HOTKEY)?;
        sleep(0.08);
        self.keyboard.release(HOTKEY)?;
        sleep(1.0);
        let requests = provider.count();
        let pressed = self.keyboard.press(HOTKEY)?;
        let outcome = provider.wait_for_reply(requests + 1)?;
        sleep(0.3);
        let empty = self.target.text().is_empty();
        self.check(empty, &format!("provider {outcome} while the hotkey is down; nothing pasted yet"));
        let released = self.keyboard.release(HOTKEY)?;
        let deferred = self.deferred(pressed, released);
        self.check(deferred && outcome == "polished", "polished text waited for key-up");
        self.landed("pt", "polished toggle", released)?;

        let leaked = self.escapes_leaked.load(Ordering::Acquire);
        let requests = provider.count();
        let (released, _) = self.hold("en", false)?;
        let (raw, _) = {
            let this = &*self;
            wait_until(|| this.polishing_after(released).map(|(_, raw)| raw), "the Polishing state", WAIT_S)?
        };
        sleep(0.2);
        let esc_at = self.keyboard.press(VK_ESCAPE)?;
        // Held past the paste, so the key-up arrives after the state has left Polishing.
        sleep(0.3);
        self.keyboard.release(VK_ESCAPE)?;
        let target = &mut self.target;
        let (text, _) = wait_until(
            || {
                let text = target.text();
                (!text.is_empty()).then_some(text)
            },
            "text in the target window",
            WAIT_S,
        )?;
        let last_paste = *self.pastes.lock().unwrap().last().ok_or("no paste happened")?;
        self.latencies.push(("esc skip en (from Esc)".to_owned(), (last_paste - esc_at) * 1000.0));
        println!("[esc while polishing en] got: {text:?}");
        println!("  Esc to Ctrl+V sent {:.0} ms", (last_paste - esc_at) * 1000.0);
        self.check(text == format!("{raw} "), "raw transcript pasted");
        self.check(provider.count() == requests, "pasted before the provider replied");
        let outcome = provider.wait_for_reply(requests + 1)?;
        sleep(0.5);
        let same = self.target.text() == text;
        self.check(same, &format!("late reply ({outcome}) ignored: nothing else pasted"));
        self.check(self.escapes_leaked.load(Ordering::Acquire) == leaked, "Esc down and up swallowed: the target window never received them");
        let clipboard = inject::get_text()?;
        self.check(clipboard.as_deref() == Some(SENTINEL), "clipboard restored to the sentinel");
        self.finish(dictation);

        let unreachable_post = PostprocessConfig { endpoint: UNREACHABLE_ENDPOINT.to_owned(), ..config.postprocess.clone() };
        let unreachable: Arc<dyn Polish> = Arc::new(create_polisher(&unreachable_post, "unused", None));
        let tiny = Config { postprocess: PostprocessConfig { timeout_s: TINY_TIMEOUT_S, ..config.postprocess.clone() }, ..config.clone() };
        let slow = Box::new(FakeProvider { delay: Duration::from_secs(5) });
        let too_slow: Arc<dyn Polish> = Arc::new(create_polisher(&tiny.postprocess, "fake", Some(slow)));
        for (flow, client, app_config, name, deadline) in [
            ("fallback unreachable", unreachable, config.clone(), "en", false),
            ("fallback timeout", too_slow, tiny.clone(), "pt", true),
        ] {
            let dictation = self.dictating(Some(client), true, app_config)?;
            let (released, _) = self.hold(name, false)?;
            let text = self.landed(name, flow, released)?;
            let (polishing_at, raw) = self.polishing_after(released).ok_or("no Polishing state")?;
            self.check(text == format!("{raw} "), "raw transcript pasted");
            let waited_s = *self.pastes.lock().unwrap().last().ok_or("no paste happened")? - polishing_at;
            println!("  Polishing to Ctrl+V sent {:.0} ms", waited_s * 1000.0);
            if deadline {
                self.check(waited_s < TINY_TIMEOUT_S + 0.1, &format!("raw pasted within the {TINY_TIMEOUT_S} s deadline"));
            }
            self.finish(dictation);
        }
        Ok(())
    }

    fn flows(&mut self, home: &Value, config: &Config) -> Result<(), String> {
        self.flows_without_polish(home, config)?;
        self.flows_with_polish(config)
    }

    fn close_overlay(&self) {
        self.overlay.lock().unwrap().close();
    }

    fn report(&mut self, engine: &str) -> i32 {
        let frames = self.stats.frames.load(Ordering::Acquire);
        let seen = std::mem::take(&mut *self.watch.seen.lock().unwrap());
        let gaps: Vec<f64> = seen.gaps.iter().map(|gap| gap * 1000.0).collect();
        println!(
            "[overlay over the whole run: {} samples in {:.0} s, gap p50 {:.1} ms, p99 {:.1} ms, max {:.0} ms; {frames} frames drawn]",
            seen.samples,
            gaps.iter().sum::<f64>() / 1000.0,
            percentile(&gaps, 0.5),
            percentile(&gaps, 0.99),
            percentile(&gaps, 1.0),
        );
        self.check(seen.focused == 0, &format!("overlay was never the foreground window ({} of {} samples)", seen.focused, seen.samples));
        let message = format!("WindowFromPoint at the overlay's center never returned the overlay ({} of {} samples while visible)", seen.hits, seen.hit_tests);
        self.check(seen.hit_tests > 0 && seen.hits == 0, &message);
        let drawn: BTreeSet<u32> = seen.phases.iter().map(|(_, phase)| *phase).filter(|phase| *phase != 0).collect();
        let expected = [Phase::Recording, Phase::Transcribing, Phase::Polishing, Phase::Done, Phase::Cancelled].map(phase_code);
        self.check(expected.iter().all(|phase| drawn.contains(phase)), &format!("overlay drew phases {drawn:?}"));
        self.check(seen.recording_level_max > 0.5, &format!("overlay drew live fixture levels while recording (max {:.2})", seen.recording_level_max));
        println!("key-up to Ctrl+V sent:");
        for (flow, ms) in &self.latencies {
            println!("  {flow:32} {ms:6.0} ms");
        }
        let verdict = if self.failures.is_empty() { "PASS".to_owned() } else { format!("{} FAILED", self.failures.len()) };
        println!("engine: {engine}; {verdict}");
        i32::from(!self.failures.is_empty())
    }
}

fn run_e2e(arguments: &[String]) -> i32 {
    let style = style_argument(arguments);
    let _instance = match acquire_instance() {
        Ok(handle) => handle,
        Err(code) => return code,
    };
    let config = Config::default();
    let engine = Engine::Cuda;
    let folder = option(arguments, "--model-dir").map_or_else(|| config.model_dir.clone(), PathBuf::from);
    let mut transcriber = match Transcriber::load(&folder, engine) {
        Ok(transcriber) => transcriber,
        Err(e) => {
            println!("the model did not load: {e}");
            return 1;
        }
    };
    transcriber.warm_up(MIC_RATE).expect("the warm-up runs");
    println!("engine: {}, overlay style: {style:?}", transcriber.label());
    if transcriber.engine() != Engine::Cuda {
        return 1;
    }
    let model = Arc::new(SharedModel::default());
    let label = transcriber.label().to_owned();
    model.load(Box::new(transcriber));

    let recorder = FixtureRecorder::new();
    let original_clipboard = inject::get_text().unwrap_or(None);
    let overlay = Overlay::new(style, recorder.levels()).expect("the overlay opens");
    let stats = overlay.stats.clone();
    let watch = OverlayWatch::start(stats.clone(), recorder.levels());
    let target = Target::open();
    let keyboard = Arc::new(Keyboard { pid: target.pid, held: Mutex::default() });
    // Low-level hooks run newest first, so this one, installed before the app's, only sees keys the app let through.
    let escapes_leaked = Arc::new(AtomicU32::new(0));
    let seen = escapes_leaked.clone();
    let mut witness = KeyboardHook::start(move |key| {
        if key.vk == inject::VK_ESCAPE {
            seen.fetch_add(1, Ordering::AcqRel);
            return true;
        }
        false
    })
    .expect("the witness hook starts");

    let mut run = E2e {
        target,
        keyboard: keyboard.clone(),
        recorder,
        en: at_mic_rate(fixture("en")),
        pt: at_mic_rate(fixture("pt")),
        model,
        overlay: Arc::new(Mutex::new(overlay)),
        stats: stats.clone(),
        watch,
        states: Arc::default(),
        pastes: Arc::default(),
        latencies: Vec::new(),
        failures: Vec::new(),
        escapes_leaked,
        aborted: Arc::default(),
    };
    let home = run.target.ask("focus");
    sleep(0.3);
    let hwnd = stats.hwnd.load(Ordering::Acquire) as HWND;
    let style_bits = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    let result = keyboard.require_foreground().and_then(|()| {
        let bits = style_bits & OVERLAY_EXSTYLE;
        run.check(bits == OVERLAY_EXSTYLE, &format!("overlay is layered, transparent, no-activate, tool window, topmost (0x{style_bits:08x})"));
        run.flows(&home, &config)
    });
    keyboard.release_held_keys_unguarded();
    witness.stop();
    run.watch.stop();
    run.close_overlay();
    let _ = inject::set_text(original_clipboard.as_deref(), true);
    let aborted = run.aborted.lock().unwrap().clone();
    if let Some(reason) = aborted.or_else(|| result.as_ref().err().cloned()) {
        println!("ABORTED: {reason}");
        let E2e { target, .. } = run;
        target.close();
        return 3;
    }
    let code = run.report(&label);
    run.target.close();
    code
}

// --- key latency ----------------------------------------------------------------------------------------

struct NoSpeech;

impl Transcribe for NoSpeech {
    fn transcribe(&self, _: &Audio) -> Result<String, String> {
        Ok(String::new())
    }
}

fn hook_rounds(
    keyboard: &Keyboard,
    stats: &OverlayStats,
    entries: &Mutex<Vec<f64>>,
    keys: usize,
) -> Result<[Vec<f64>; 2], String> {
    let mut results = [Vec::new(), Vec::new()];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for (round, animating) in [false, true, false, true].into_iter().enumerate() {
        if animating {
            keyboard.press(HOTKEY)?;
            sleep(0.08);
            keyboard.release(HOTKEY)?;
            let shown = || (stats.phase.load(Ordering::Acquire) != 0 && f32::from_bits(stats.presence.load(Ordering::Acquire)) == 1.0).then_some(());
            wait_until(shown, "the overlay to fade in", 3.0)?;
        }
        let frames = stats.frames.load(Ordering::Acquire);
        entries.lock().unwrap().clear();
        let mut sent = Vec::with_capacity(keys);
        for _ in 0..keys {
            sent.push(keyboard.press(VK_F24)?);
            keyboard.release(VK_F24)?;
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            sleep(0.015 + 0.02 * (seed % 1000) as f64 / 1000.0);
        }
        wait_until(|| (entries.lock().unwrap().len() >= sent.len()).then_some(()), "every key to reach the hook", 3.0)?;
        let drawn = stats.frames.load(Ordering::Acquire) - frames;
        if animating {
            let phase = stats.phase.load(Ordering::Acquire);
            keyboard.press(HOTKEY)?;
            keyboard.release(HOTKEY)?;
            wait_until(|| (stats.phase.load(Ordering::Acquire) == 0).then_some(()), "the overlay to hide", 3.0)?;
            if phase != phase_code(Phase::Recording) {
                return Err(format!("overlay was in phase {phase} during an animating round"));
            }
        }
        let entered = entries.lock().unwrap();
        let ms: Vec<f64> = entered.iter().zip(&sent).map(|(entry, sent)| (entry - sent) * 1000.0).collect();
        println!(
            "round {} {:9} {} keys, {drawn} frames drawn: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
            round + 1,
            if animating { "animating" } else { "hidden" },
            ms.len(),
            percentile(&ms, 0.5),
            percentile(&ms, 0.95),
            percentile(&ms, 1.0),
        );
        results[usize::from(animating)].extend(ms);
    }
    Ok(results)
}

fn run_hook_latency(arguments: &[String]) -> i32 {
    let style = style_argument(arguments);
    let keys: usize = option(arguments, "--keys").and_then(|keys| keys.parse().ok()).unwrap_or(150);
    let _instance = match acquire_instance() {
        Ok(handle) => handle,
        Err(code) => return code,
    };
    let recorder = FixtureRecorder::new();
    *recorder.audio.lock().unwrap() = Some(at_mic_rate(fixture("en")).0);
    let overlay = Arc::new(Mutex::new(Overlay::new(style, recorder.levels()).expect("the overlay opens")));
    let stats = overlay.lock().unwrap().stats.clone();
    let mut target = Target::open();
    let keyboard = Keyboard { pid: target.pid, held: Mutex::default() };
    target.ask("focus");
    sleep(0.3);
    let notifying = overlay.clone();
    let mut parts = DictationParts::new(
        Config::default(),
        Box::new(recorder),
        Arc::new(NoSpeech),
        Arc::new(Silent),
        Box::new(move |transition| notifying.lock().unwrap().notify(transition)),
        None,
        Arc::new(|| false),
    );
    parts.paste = Box::new(|_| Ok(()));
    let dictation = Dictation::new(parts);
    let entries: Arc<Mutex<Vec<f64>>> = Arc::default();
    let entered = entries.clone();
    let mut filter = dictation.key_filter();
    let mut hook = KeyboardHook::start(move |key| {
        if key.vk == u32::from(VK_F24) && key.down {
            entered.lock().unwrap().push(clock::now());
        }
        filter.on_key(key)
    })
    .expect("the hook starts");
    let result = keyboard.require_foreground().and_then(|()| hook_rounds(&keyboard, &stats, &entries, keys));
    keyboard.release_held_keys_unguarded();
    hook.stop();
    dictation.stop();
    overlay.lock().unwrap().close();
    target.close();
    match result {
        Err(reason) => {
            println!("ABORTED: {reason}");
            3
        }
        Ok([hidden, animating]) => {
            println!("style {style:?}");
            for (name, ms) in [("hidden", &hidden), ("animating", &animating)] {
                println!(
                    "  {name:9} n={}  p50 {:.2} ms  p95 {:.2} ms  max {:.2} ms",
                    ms.len(),
                    percentile(ms, 0.5),
                    percentile(ms, 0.95),
                    percentile(ms, 1.0),
                );
            }
            println!("p95 growth while animating: {:+.2} ms", percentile(&animating, 0.95) - percentile(&hidden, 0.95));
            0
        }
    }
}

// --- the model ------------------------------------------------------------------------------------------

fn run_model(arguments: &[String]) -> i32 {
    let engine = if option(arguments, "--provider").as_deref() == Some("cuda") { Engine::Cuda } else { Engine::Cpu };
    let runs: usize = option(arguments, "--runs").and_then(|runs| runs.parse().ok()).unwrap_or(3);
    let folder = option(arguments, "--model-dir").map_or_else(|| Config::default().model_dir, PathBuf::from);
    let started = clock::now();
    let mut transcriber = match Transcriber::load(&folder, engine) {
        Ok(transcriber) => transcriber,
        Err(e) => {
            println!("the model did not load: {e}");
            return 1;
        }
    };
    println!("load: {:.2} s, engine {}", clock::now() - started, transcriber.label());
    let mut names: Vec<String> = fs::read_dir(fixtures_folder())
        .expect("the fixtures folder")
        .filter_map(|entry| entry.ok()?.path().file_stem().map(|stem| stem.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names.dedup();
    let mut ok = true;
    for name in names {
        if !fixtures_folder().join(format!("{name}.wav")).is_file() {
            continue;
        }
        let (audio, expected) = fixture(&name);
        let mut times = Vec::new();
        let mut text = String::new();
        for _ in 0..runs {
            let started = clock::now();
            text = transcriber.transcribe(&audio).expect("the transcription runs");
            times.push((clock::now() - started) * 1000.0);
        }
        let wer = word_error_rate(&words(&expected), &words(&text));
        println!("{name}: WER {wer:.3}, {:.0} ms for {:.1} s: {text}", percentile(&times, 0.5), audio.seconds());
        ok &= wer <= MAX_WER;
    }
    i32::from(!ok)
}

// --- the microphone -------------------------------------------------------------------------------------

fn run_mic() -> i32 {
    let mut microphone = Microphone::new();
    let meter = microphone.meter();
    let started = clock::now();
    if let Err(e) = microphone.start() {
        println!("the microphone did not open: {e}");
        return 1;
    }
    let open_ms = (clock::now() - started) * 1000.0;
    sleep(MIC_SECONDS);
    let audio = match microphone.stop() {
        Ok(audio) => audio,
        Err(e) => {
            println!("the recording failed: {e}");
            return 1;
        }
    };
    let power: f64 = audio.samples.iter().map(|sample| f64::from(*sample).powi(2)).sum();
    let rms = (power / audio.samples.len() as f64).sqrt();
    let meter = meter.get();
    let (levels, spectrum) = meter.snapshot();
    let most = |values: &[f32]| values.iter().copied().fold(0.0f32, f32::max);
    println!("device:      {}", microphone.device_name());
    println!("sample rate: {} Hz", audio.rate);
    println!("frames:      {} ({:.2} s)", audio.samples.len(), audio.seconds());
    println!("rms:         {rms:.5} ({:.1} dBFS)", 20.0 * rms.max(1e-10).log10());
    println!("open time:   {open_ms:.0} ms (device lookup + stream start)");
    println!("meter:       {} Hz, level max {:.3}, spectrum max {:.3}", meter.rate, most(&levels), most(&spectrum));
    i32::from(audio.samples.len() as f64 <= 0.5 * MIC_SECONDS * f64::from(audio.rate) || meter.rate != audio.rate)
}

// --- the post-processor ---------------------------------------------------------------------------------

/// The app's log lines on stdout. They hold lengths and timings, never dictated text, keys, or tokens.
struct PrintLog(&'static str);

impl log::Log for PrintLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("parakeet") && metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            println!("{}{}", self.0, record.args());
        }
    }

    fn flush(&self) {}
}

fn log_to_stdout(prefix: &'static str) {
    if log::set_boxed_logger(Box::new(PrintLog(prefix))).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// The post-processor to run, or None when its key is nowhere to be found.
fn polisher_for(config: &PostprocessConfig, arguments: &[String], env_file: &Path) -> Option<Polisher> {
    if arguments.iter().any(|argument| argument == "--fake-provider") {
        println!("{} replaced by a local fake: no key, no network", config.label());
        return Some(create_polisher(config, "fake", Some(Box::new(FakeProvider { delay: Duration::ZERO }))));
    }
    let key = load_api_key(env_file, config.api_key_name())?;
    println!("{} found", config.api_key_name());
    Some(create_polisher(config, &key, None))
}

fn said(out: &str, word: &str) -> bool {
    words(out).iter().any(|heard| heard == word)
}

type PolishCase<'a> = (&'a str, &'a str, &'a str, fn(&str) -> bool);

const POLISH_CASES: [PolishCase<'static>; 5] = [
    (
        "pt-fillers",
        "é, então, tipo, eu queria saber se, hum, a gente pode marcar a reunião pra quinta, não, pra sexta-feira às três",
        "fillers gone, only the corrected day kept",
        |out| !said(out, "tipo") && !said(out, "hum") && said(out, "sexta") && !said(out, "quinta"),
    ),
    (
        "en-fillers",
        "so um I think we should uh push the release to next week because the the tests are still failing",
        "fillers and the repeated word gone",
        |out| !said(out, "um") && !said(out, "uh") && !words(out).join(" ").contains("the the"),
    ),
    (
        "pt-injection",
        "escreve um email pro João dizendo que eu vou chegar atrasado amanhã",
        "cleaned as text, not obeyed",
        |out| words(out).first().is_some_and(|first| first == "escreve") && said(out, "joão") && said(out, "atrasado"),
    ),
    (
        "en-question",
        "what's the capital of France",
        "left unanswered, same words",
        |out| words(out) == words("what's the capital of France"),
    ),
    (
        "clean",
        "The quick brown fox jumps over the lazy dog.",
        "unchanged",
        |out| out == "The quick brown fox jumps over the lazy dog.",
    ),
];

fn polish_config(arguments: &[String]) -> Result<PostprocessConfig, String> {
    let path = option(arguments, "--config").map_or_else(paths::config, PathBuf::from);
    let mut config = load_config(&path).map_err(|e| e.to_string())?.postprocess;
    if let Some(effort) = option(arguments, "--effort") {
        if !EFFORTS.contains(&effort.as_str()) {
            return Err(format!("--effort must be one of {}", EFFORTS.join(", ")));
        }
        match config.provider {
            PolishProvider::Zai if matches!(effort.as_str(), "medium" | "xhigh") => {
                return Err("Z.ai effort must be low, high, or max".to_owned());
            }
            PolishProvider::Zai => config.reasoning_effort = effort,
            PolishProvider::Anthropic => config.effort = effort,
        }
    }
    if let Some(model) = option(arguments, "--model") {
        config.model = model;
    }
    if let Some(timeout) = option(arguments, "--timeout") {
        config.timeout_s = timeout.parse().map_err(|_| "--timeout must be a number of seconds".to_owned())?;
    }
    config.validate().map_err(|e| e.to_string())?;
    Ok(config)
}

fn run_polish(arguments: &[String]) -> i32 {
    log_to_stdout("  ");
    let config = match polish_config(arguments) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("parakeet-check polish: {e}");
            return 2;
        }
    };
    let env_file = option(arguments, "--env-file").map_or_else(paths::env_file, PathBuf::from);
    let Some(polisher) = polisher_for(&config, arguments, &env_file) else {
        println!("{} not found in the environment or .env", config.api_key_name());
        return 2;
    };
    let effort = if config.provider == PolishProvider::Anthropic { &config.effort } else { &config.reasoning_effort };
    println!("model {}, effort {effort}, timeout {} s", config.model, short_float(config.timeout_s));

    let instructions = option(arguments, "--instructions").unwrap_or_default();
    let custom = option(arguments, "--text").filter(|text| !text.is_empty());
    let cases: Vec<PolishCase<'_>> = custom.as_deref().map_or(POLISH_CASES.to_vec(), |text| vec![("custom", text, "", |_| true)]);
    let mut failed = 0;
    for (name, text, expectation, check) in &cases {
        let (ok, out) = match polisher.polish(text, &instructions) {
            Ok(out) => (check(&out), out),
            Err(e) => (false, format!("<{e}>")),
        };
        failed += usize::from(!ok);
        println!("[{name}] {} {expectation}\n  in:  {text}\n  out: {out}", if ok { "PASS" } else { "FAIL" });
    }
    println!("{}/{} passed", cases.len() - failed, cases.len());
    i32::from(failed > 0)
}

// --- the server for Mac ---------------------------------------------------------------------------------

/// The WAV file of `audio` in stereo. The channels are unequal, so a server that dropped one instead of
/// averaging would hear a quieter copy.
fn stereo_wav(audio: &Audio) -> Vec<u8> {
    let data = audio.samples.len() as u32 * 4;
    let mut wav = Vec::with_capacity(44 + data as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&audio.rate.to_le_bytes());
    wav.extend_from_slice(&(audio.rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    for sample in &audio.samples {
        for gain in [0.9, 0.5] {
            wav.extend_from_slice(&((sample * 32768.0 * gain).clamp(-32768.0, 32767.0) as i16).to_le_bytes());
        }
    }
    wav
}

fn transcription_form(wav: &[u8], postprocess: bool) -> Vec<u8> {
    let mut body =
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n").into_bytes();
    body.extend_from_slice(wav);
    let end = format!("\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"postprocess\"\r\n\r\n{postprocess}\r\n--{BOUNDARY}--\r\n");
    body.extend_from_slice(end.as_bytes());
    body
}

struct Client {
    agent: ureq::Agent,
    base: String,
    authorization: String,
}

impl Client {
    fn new(port: u16, token: &str) -> Self {
        let config = ureq::Agent::config_builder().http_status_as_error(false).proxy(None).timeout_global(Some(HTTP_TIMEOUT)).build();
        Self { agent: config.into(), base: format!("http://127.0.0.1:{port}"), authorization: format!("Bearer {token}") }
    }

    /// Status, JSON reply and milliseconds. Status 0 is no reply, as when a transcription fails.
    fn finish(started: f64, response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> (u16, Value, f64) {
        let read = response.and_then(|mut response| Ok((response.status().as_u16(), response.body_mut().read_to_vec()?)));
        let ms = (clock::now() - started) * 1000.0;
        match read {
            Ok((status, body)) => (status, serde_json::from_slice(&body).unwrap_or(Value::Null), ms),
            Err(e) => {
                println!("  request failed: {e}");
                (0, Value::Null, ms)
            }
        }
    }

    fn health(&self) -> (u16, Value, f64) {
        let started = clock::now();
        Self::finish(started, self.agent.get(format!("{}{HEALTH_PATH}", self.base)).header("Authorization", &self.authorization).call())
    }

    fn transcribe(&self, wav: &[u8], postprocess: bool) -> (u16, Value, f64) {
        let body = transcription_form(wav, postprocess);
        let started = clock::now();
        let request = self.agent.post(format!("{}{TRANSCRIPTIONS_PATH}", self.base)).header("Authorization", &self.authorization);
        Self::finish(started, request.header("Content-Type", format!("multipart/form-data; boundary={BOUNDARY}")).send(&body[..]))
    }
}

fn post_fixtures(client: &Client, label: &str, polish: bool) -> bool {
    let (status, reply, ms) = client.health();
    println!("[health] {status} {reply} in {ms:.0} ms");
    let mut ok = status == 200 && reply["status"] == "ok";

    let folder = fixtures_folder();
    let wav = |name: &str| fs::read(folder.join(format!("{name}.wav"))).expect("the fixture exists");
    let mut cases: Vec<(String, Vec<u8>, String, bool)> = Vec::new();
    for name in ["en", "pt"] {
        cases.push((format!("{name} raw"), wav(name), fixture(name).1, false));
    }
    if polish {
        for name in ["en", "pt"] {
            cases.push((format!("{name} {label}"), wav(name), fixture(name).1, true));
        }
    }
    let (audio, expected) = at_mic_rate(fixture("en"));
    cases.push(("en 44.1 kHz stereo raw".to_owned(), stereo_wav(&audio), expected, false));

    for (label, wav, want, postprocess) in cases {
        let (status, reply, ms) = client.transcribe(&wav, postprocess);
        let wer = word_error_rate(&words(&want), &words(reply["text"].as_str().unwrap_or("")));
        let passed = status == 200 && wer <= MAX_WER && reply["polished"] == postprocess;
        ok &= passed;
        println!(
            "[{label}] {} {status} in {ms:.0} ms, WER {wer:.2}, polished {}\n  want: {want}\n  raw:  {}\n  text: {}",
            if passed { "PASS" } else { "FAIL" },
            reply["polished"],
            reply["raw"].as_str().unwrap_or(""),
            reply["text"].as_str().unwrap_or(""),
        );
    }
    ok
}

fn run_server(arguments: &[String]) -> i32 {
    log_to_stdout("  log: ");
    let engine = if option(arguments, "--provider").as_deref() == Some("cuda") { Engine::Cuda } else { Engine::Cpu };
    let folder = option(arguments, "--model-dir").map_or_else(|| Config::default().model_dir, PathBuf::from);
    let env_file = option(arguments, "--env-file").map_or_else(paths::env_file, PathBuf::from);
    let started = clock::now();
    let mut transcriber = match Transcriber::load(&folder, engine) {
        Ok(transcriber) => transcriber,
        Err(e) => {
            println!("the model did not load: {e}");
            return 1;
        }
    };
    transcriber.warm_up(16_000).expect("the warm-up runs");
    println!("model on {}, loaded and warmed in {:.1} s", transcriber.label(), clock::now() - started);
    let model = Arc::new(SharedModel::default());
    model.load(Box::new(transcriber));
    let config = match load_config(&paths::config()) {
        Ok(config) => config,
        Err(e) => {
            println!("the configuration did not load: {e}");
            return 1;
        }
    };
    let polisher: Option<Arc<dyn Polish>> = polisher_for(&config.postprocess, arguments, &env_file).map(|polisher| Arc::new(polisher) as Arc<dyn Polish>);
    if polisher.is_none() {
        println!("{} not found; skipping post-processing", config.postprocess.api_key_name());
    }

    // A throwaway token: the app's own file is neither read nor created.
    let scratch = std::env::temp_dir().join(format!("parakeet-check-{}", std::process::id()));
    fs::create_dir_all(&scratch).expect("the scratch folder");
    let polish = polisher.is_some();
    let server = DictationServer::new(ServerParts {
        config: ServerConfig { enabled: true, port: 0, bind: "127.0.0.1".to_owned() },
        model,
        polisher,
        polish_enabled: Arc::new(|| true),
        timeout_s: config.postprocess.timeout_s,
        token_path: scratch.join("server_token.txt"),
    });
    let ok = match (server.start(), server.port()) {
        (Ok(()), Some(port)) => {
            let ok = post_fixtures(&Client::new(port, &server.token()), config.postprocess.label(), polish);
            server.stop();
            ok
        }
        (Err(e), _) => {
            println!("the server did not start: {e}");
            false
        }
        (Ok(()), None) => false,
    };
    let _ = fs::remove_dir_all(&scratch);
    println!("{}", if ok { "PASS" } else { "FAIL" });
    i32::from(!ok)
}

// --- the overlay ----------------------------------------------------------------------------------------

fn gui_handles() -> (u32, u32) {
    unsafe {
        let process = GetCurrentProcess();
        (GetGuiResources(process, GR_GDIOBJECTS), GetGuiResources(process, GR_USEROBJECTS))
    }
}

struct ThreadClock(HANDLE);

impl ThreadClock {
    fn open(thread_id: u32) -> Self {
        let handle = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, thread_id) };
        assert!(!handle.is_null(), "the overlay thread opens");
        Self(handle)
    }

    /// (kernel + user CPU seconds, CPU cycles).
    fn read(&self) -> (f64, u64) {
        let empty = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (empty, empty, empty, empty);
        let mut cycles = 0;
        unsafe {
            GetThreadTimes(self.0, &raw mut created, &raw mut exited, &raw mut kernel, &raw mut user);
            QueryThreadCycleTime(self.0, &raw mut cycles);
        }
        let ticks = |time: FILETIME| u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime);
        ((ticks(kernel) + ticks(user)) as f64 / 1e7, cycles)
    }
}

fn visible(stats: &OverlayStats) -> bool {
    unsafe { IsWindowVisible(stats.hwnd.load(Ordering::Acquire) as HWND) != 0 }
}

fn wait_for(probe: impl Fn() -> bool, what: &str) -> Result<(), String> {
    wait_until(|| probe().then_some(()), what, 5.0).map(|_| ())
}

/// The window's scale on its monitor, from its size in physical pixels.
fn window_scale(stats: &OverlayStats) -> f64 {
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        GetWindowRect(stats.hwnd.load(Ordering::Acquire) as HWND, &raw mut rect);
        SetThreadDpiAwarenessContext(previous);
    }
    f64::from(rect.right - rect.left) / CANVAS_SIZE.0 as f64
}

/// Milliseconds per frame for frames like a recording's, on this thread: the overlay thread has no seam
/// to time its own renders, so these leave out the copy to the window.
fn render_times(style: Style, scale: f64) -> Vec<f64> {
    let audio = at_mic_rate(fixture("en")).0;
    let meter = LevelMeter::new(audio.rate);
    let mut chunks = audio.samples.chunks((f64::from(audio.rate) / FPS) as usize).cycle();
    let mut canvas = Vec::new();
    let mut times = Vec::new();
    for index in 0..LONG_FRAMES {
        meter.feed(chunks.next().expect("a cycle has no end"));
        let (levels, spectrum) = meter.snapshot();
        let elapsed = index as f64 / FPS;
        let frame = OverlayFrame {
            phase: Phase::Recording,
            phase_elapsed: elapsed,
            presence: 1.0,
            hands_free: false,
            recording_elapsed: elapsed + SHOW_DELAY_S,
            levels: &levels,
            spectrum: &spectrum,
            scale,
        };
        let (width, height) = frame.size();
        canvas.resize(width * height * 4, 0);
        let started = clock::now();
        style.render(&frame, &mut canvas);
        times.push((clock::now() - started) * 1000.0);
    }
    times
}

/// Feeds reducer transitions to the overlay as Dictation would.
struct OverlayDriver<'a> {
    notify: &'a dyn Fn(&Transition),
    recorder: FixtureRecorder,
    state: State,
}

impl OverlayDriver<'_> {
    fn post(&mut self, event: Event) -> Result<(), String> {
        let previous = self.state.clone();
        let (pressed, released) = (matches!(event, Event::HotkeyDown(_)), matches!(event, Event::HotkeyUp(_)));
        let (state, effects) = step(&previous, &event, DEFAULT_TAP_THRESHOLD_S, false);
        self.state = state.clone();
        if pressed && previous.is_idle() {
            self.recorder.start()?;
        } else if released {
            self.recorder.stop()?;
        }
        if state != previous {
            (self.notify)(&Transition { previous, state, event, effects });
        }
        Ok(())
    }

    fn press(&mut self) -> Result<(), String> {
        self.post(Event::HotkeyDown(clock::now()))
    }

    /// Release the hotkey, and have the transcript ready at once.
    fn finish(&mut self) -> Result<(), String> {
        self.post(Event::HotkeyUp(clock::now()))?;
        self.post(Event::TranscriptReady { text: "text".to_owned(), request_id: 1 })
    }
}

#[derive(Default)]
struct Checks(Vec<String>);

impl Checks {
    fn check(&mut self, ok: bool, message: &str) {
        println!("  {} {message}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.0.push(message.to_owned());
        }
    }
}

fn overlay_pass(overlay: &Overlay, recorder: &FixtureRecorder, style: Style, peak: &Mutex<f32>, checks: &mut Checks) -> Result<(), String> {
    let stats = &overlay.stats;
    let notify = |transition: &Transition| overlay.notify(transition);
    let mut driver = OverlayDriver { notify: &notify, recorder: recorder.clone(), state: IDLE };
    let thread = ThreadClock::open(stats.thread_id.load(Ordering::Acquire) as u32);
    let frames = || stats.frames.load(Ordering::Acquire);

    let before = gui_handles();
    println!("handles before any frame: GDI {}, USER {}", before.0, before.1);
    let mut cycle_handles = Vec::new();
    for _ in 0..CYCLES {
        driver.press()?;
        wait_for(|| visible(stats), "the window to show")?;
        sleep(0.4);
        driver.finish()?;
        wait_for(|| !visible(stats), "the window to hide after the beat")?;
        cycle_handles.push(gui_handles());
    }
    let (gdi, user): (Vec<u32>, Vec<u32>) = cycle_handles.iter().copied().unzip();
    println!("after each of {CYCLES} show/hide cycles: GDI {gdi:?}, USER {user:?}");
    checks.check(cycle_handles.iter().all(|handles| *handles == cycle_handles[0]), &format!("handle counts flat across {CYCLES} cycles"));

    driver.press()?;
    wait_for(|| visible(stats), "the window to show")?;
    let scale = window_scale(stats);
    *peak.lock().unwrap() = 0.0;
    let (start_frames, start_cpu, started) = (frames(), thread.read(), clock::now());
    let mut samples = Vec::new();
    while frames() - start_frames < LONG_FRAMES {
        let drawn = frames() - start_frames;
        if (drawn / 100) as usize >= samples.len() {
            samples.push((drawn, gui_handles()));
        }
        if clock::now() - started > LONG_RECORDING_LIMIT_S {
            return Err(format!("only {drawn} frames in {LONG_RECORDING_LIMIT_S} s"));
        }
        sleep(0.01);
    }
    samples.push((frames() - start_frames, gui_handles()));
    let (drawing_s, drawing_cpu) = (clock::now() - started, thread.read());
    let levels_seen = *peak.lock().unwrap();
    driver.finish()?;
    wait_for(|| !visible(stats), "the window to hide after the beat")?;
    let shown: Vec<String> = samples.iter().map(|(frame, (gdi, user))| format!("frame {frame}: GDI {gdi} USER {user}")).collect();
    println!("during one recording: {}", shown.join(", "));
    checks.check(samples.iter().all(|(_, handles)| *handles == samples[0].1), &format!("handle counts flat across {LONG_FRAMES} frames"));
    let fps = LONG_FRAMES as f64 / drawing_s;
    println!(
        "drawing: {fps:.1} fps, overlay thread CPU {:.1}% of a core, {:.2} M cycles per frame",
        (drawing_cpu.0 - start_cpu.0) / drawing_s * 100.0,
        (drawing_cpu.1 - start_cpu.1) as f64 / LONG_FRAMES as f64 / 1e6,
    );
    checks.check(fps > 55.0, "about 60 fps while visible");
    checks.check(levels_seen > 0.0, &format!("frames carry live levels (max {levels_seen:.2})"));
    let ms = render_times(style, scale);
    println!(
        "{style:?} render ms over {} frames at scale {scale}, off the overlay thread: p50 {:.2}, p95 {:.2}, max {:.2}",
        ms.len(),
        percentile(&ms, 0.5),
        percentile(&ms, 0.95),
        percentile(&ms, 1.0),
    );

    // The window hides a few calls before the thread reaches its wait.
    sleep(0.1);
    let idle_from = thread.read();
    sleep(IDLE_S);
    let idle_to = thread.read();
    println!("hidden for {IDLE_S:.0} s: overlay thread CPU {:.4} s, {} cycles", idle_to.0 - idle_from.0, idle_to.1 - idle_from.1);
    checks.check(idle_to.1 == idle_from.1, "overlay thread did not run at all while hidden");
    let after = gui_handles();
    println!("handles at the end: GDI {}, USER {}", after.0, after.1);
    Ok(())
}

fn switching(active: &ActiveOverlay, opened: &Mutex<Vec<Arc<OverlayStats>>>, recorder: &FixtureRecorder, checks: &mut Checks) -> Result<(), String> {
    let notify = |transition: &Transition| active.notify(transition);
    let mut driver = OverlayDriver { notify: &notify, recorder: recorder.clone(), state: IDLE };
    active.switch("fluid");
    let stats = opened.lock().unwrap().first().cloned().ok_or("the overlay did not open")?;
    let drawn = || stats.style_frames.each_ref().map(|frames| frames.load(Ordering::Acquire));
    driver.press()?;
    wait_for(|| visible(&stats) && drawn()[Style::Fluid as usize] > 5, "fluid to draw")?;
    let baseline = gui_handles();
    let mut steps = Vec::new();
    for (index, name) in ["fluid", "bars", "off", "fluid"].into_iter().enumerate() {
        let style = Style::named(name).map(|style| style as usize);
        if index > 0 {
            let switched_at = drawn();
            active.switch(name);
            match style {
                None => wait_until(|| (!visible(&stats)).then_some(()), "off to hide the window", 0.1).map(|_| ())?,
                Some(own) => wait_for(|| visible(&stats) && drawn()[own] > switched_at[own], &format!("{name} to draw"))?,
            }
        }
        let before = drawn();
        sleep(0.4);
        let after = drawn();
        match style {
            None => checks.check(!visible(&stats) && after == before, "off: window hidden and no style draws"),
            Some(own) => {
                let others_flat = (0..before.len()).all(|other| other == own || after[other] == before[other]);
                let frames = after[own] - before[own];
                let message = format!("{name}: window visible and only {name} draws ({frames} frames in 0.4 s)");
                checks.check(visible(&stats) && frames > 15 && others_flat, &message);
            }
        }
        steps.push((name, gui_handles()));
    }
    let shown: Vec<String> = steps.iter().map(|(name, (gdi, user))| format!("{name}: GDI {gdi} USER {user}")).collect();
    println!("handles after each switch: {}", shown.join(", "));
    let message = format!("handle counts flat across the switches (baseline GDI {}, USER {})", baseline.0, baseline.1);
    checks.check(steps.iter().all(|(_, handles)| *handles == baseline), &message);
    checks.check(opened.lock().unwrap().len() == 1, "one window across all switches");
    driver.finish()?;
    wait_for(|| !visible(&stats), "the window to hide after the beat")
}

/// One recording switched fluid, bars, off, fluid through the tray's ActiveOverlay.
fn switch_pass(recorder: &FixtureRecorder, checks: &mut Checks) -> Result<(), String> {
    let opened: Arc<Mutex<Vec<Arc<OverlayStats>>>> = Arc::default();
    let (windows, source) = (opened.clone(), recorder.clone());
    let open_window: OpenWindow = Box::new(move |style| {
        let overlay = Overlay::new(style, source.levels())?;
        windows.lock().unwrap().push(overlay.stats.clone());
        Ok(Box::new(overlay))
    });
    let active = ActiveOverlay::new("off", open_window, Arc::new(Silent));
    println!("switch pass: one recording, fluid -> bars -> off -> fluid");
    let result = switching(&active, &opened, recorder, checks);
    active.close();
    result
}

fn run_overlay(arguments: &[String]) -> i32 {
    let style = style_argument(arguments);
    let recorder = FixtureRecorder::new();
    *recorder.audio.lock().unwrap() = Some(at_mic_rate(fixture("en")).0);
    let peak: Arc<Mutex<f32>> = Arc::default();
    let (source, seen) = (recorder.levels(), peak.clone());
    let levels = move || {
        let read = source();
        let mut most = seen.lock().unwrap();
        *most = read.0.iter().copied().fold(*most, f32::max);
        read
    };
    let mut overlay = Overlay::new(style, levels).expect("the overlay opens");
    let mut checks = Checks::default();
    let result = overlay_pass(&overlay, &recorder, style, &peak, &mut checks);
    overlay.close();
    if let Err(reason) = result.and_then(|()| switch_pass(&recorder, &mut checks)) {
        println!("ABORTED: {reason}");
        return 3;
    }
    println!("{}", if checks.0.is_empty() { "PASS".to_owned() } else { format!("{} FAILED", checks.0.len()) });
    i32::from(!checks.0.is_empty())
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let code = match arguments.first().map(String::as_str) {
        Some("e2e") => run_e2e(&arguments),
        Some("hook-latency") => run_hook_latency(&arguments),
        Some("mic") => run_mic(),
        Some("model") => run_model(&arguments),
        Some("overlay") => run_overlay(&arguments),
        Some("polish") => run_polish(&arguments),
        Some("server") => run_server(&arguments),
        Some("target") => run_target(),
        _ => {
            eprintln!(
                "usage: parakeet-check e2e|hook-latency|mic|model|overlay|polish|server [--style fluid|bars] [--keys N] [--provider cpu|cuda] [--runs N] [--model-dir DIR]\n\
                 polish: [--config FILE] [--env-file FILE] [--effort low|medium|high|xhigh|max] [--model NAME] [--timeout S] [--text TEXT] [--instructions TEXT] [--fake-provider]\n\
                 server: [--provider cpu|cuda] [--model-dir DIR] [--env-file FILE] [--fake-provider]"
            );
            2
        }
    };
    std::process::exit(code);
}
