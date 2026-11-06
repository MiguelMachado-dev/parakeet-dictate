//! Port of tests/test_tray.py. `run` stands in for the `fake_main` fixture.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use parakeet_dictate::app::{Machine, Names, ReadyModel, RunningDictation, TrayLoop, parse_arguments, run_app};
use parakeet_dictate::config::{Config, ConfigError, Engine, PolishProvider, PostprocessConfig, ServerConfig};
use parakeet_dictate::cues::PlayCue;
use parakeet_dictate::dictation::DictationParts;
use parakeet_dictate::instance::{LAUNCH_EVENT_NAME, MUTEX_NAME, QUIT_EVENT_NAME, RELOAD_EVENT_NAME, signal_event};
use parakeet_dictate::meter::{Levels, Spectrum};
use parakeet_dictate::overlay::Style;
use parakeet_dictate::overlay::active::{ActiveOverlay, OpenWindow};
use parakeet_dictate::overlay::window::OverlaySurface;
use parakeet_dictate::polish::{Headers, Reply, Transport, TransportError};
use parakeet_dictate::recorder::Audio;
use parakeet_dictate::reducer::{Cue, Event, IDLE, Mode, State, Transition};
use parakeet_dictate::server::{DictationServer, HEALTH_PATH, ServerParts, TRANSCRIPTIONS_PATH};
use parakeet_dictate::transcriber::{LoadedModel, SharedModel};
use parakeet_dictate::tray::{Glyph, MenuItem, Tray, TrayParts, TrayShell};
use parakeet_dictate::tray_choices::TrayChoices;
use parakeet_dictate::tray_window::glyph_pixels;
use serde_json::{Value, json};
use tempfile::TempDir;

fn recording() -> Transition {
    Transition {
        previous: IDLE,
        state: State::Recording { mode: Mode::Hold, pressed_at: 0.0 },
        event: Event::HotkeyDown(0.0),
        effects: vec![],
    }
}

fn second_of_silence() -> Vec<u8> {
    let data_bytes: u32 = 32_000;
    let mut wav = Vec::with_capacity(44 + data_bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    wav.resize(wav.len() + data_bytes as usize, 0);
    wav
}

/// What one fake window was told.
#[derive(Default)]
struct WindowLog {
    renderer: Mutex<Option<Style>>,
    notified: Mutex<Vec<Transition>>,
    closed: AtomicBool,
}

impl WindowLog {
    fn renderer(&self) -> Option<Style> {
        *self.renderer.lock().unwrap()
    }

    fn notified(&self) -> Vec<Transition> {
        self.notified.lock().unwrap().clone()
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

struct FakeOverlay(Arc<WindowLog>);

impl OverlaySurface for FakeOverlay {
    fn notify(&self, transition: &Transition) {
        self.0.notified.lock().unwrap().push(transition.clone());
    }

    fn restyle(&self, style: Option<Style>) {
        *self.0.renderer.lock().unwrap() = style;
    }

    fn close(&mut self) {
        self.0.closed.store(true, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct FakeCues {
    played: Mutex<Vec<Cue>>,
}

impl FakeCues {
    fn played(&self) -> Vec<Cue> {
        self.played.lock().unwrap().clone()
    }
}

impl PlayCue for FakeCues {
    fn play(&self, cue: Cue) {
        self.played.lock().unwrap().push(cue);
    }
}

/// An overlay factory that records each window it opens.
#[derive(Clone, Default)]
struct Windows {
    opened: Arc<Mutex<Vec<Arc<WindowLog>>>>,
    fail: bool,
}

impl Windows {
    fn failing() -> Self {
        Self { fail: true, ..Self::default() }
    }

    fn open(&self, style: Style) -> Result<Box<dyn OverlaySurface>, String> {
        if self.fail {
            return Err("CreateWindowExW failed".to_owned());
        }
        let window = Arc::new(WindowLog { renderer: Mutex::new(Some(style)), ..WindowLog::default() });
        self.opened.lock().unwrap().push(window.clone());
        Ok(Box::new(FakeOverlay(window)))
    }

    fn opener(&self) -> OpenWindow {
        let windows = self.clone();
        Box::new(move |style| windows.open(style))
    }

    fn opened(&self) -> Vec<Arc<WindowLog>> {
        self.opened.lock().unwrap().clone()
    }
}

fn active_overlay(style: &str, windows: &Windows, cues: Arc<FakeCues>) -> ActiveOverlay {
    ActiveOverlay::new(style, windows.opener(), cues)
}

#[test]
fn test_starting_off_creates_no_window_and_forwards_nothing() {
    let windows = Windows::default();
    let overlay = active_overlay("off", &windows, Arc::default());
    overlay.notify(&recording());
    assert_eq!(overlay.style(), "off");
    assert!(windows.opened().is_empty());
}

#[test]
fn test_switching_opens_one_window_and_restyles_it_from_then_on() {
    let windows = Windows::default();
    let overlay = active_overlay("off", &windows, Arc::default());
    assert!(overlay.switch("bars"));
    let renderers: Vec<_> = windows.opened().iter().map(|window| window.renderer()).collect();
    assert_eq!(renderers, [Some(Style::Bars)]);
    let window = windows.opened()[0].clone();

    assert!(overlay.switch("fluid"));
    assert_eq!((overlay.style().as_str(), windows.opened().len(), window.renderer()), ("fluid", 1, Some(Style::Fluid)));
    overlay.notify(&recording());
    assert_eq!(window.notified(), [recording()]);

    assert!(overlay.switch("off"));
    assert_eq!(window.renderer(), None);
    overlay.notify(&recording());
    assert_eq!(window.notified(), [recording()], "off still forwards transitions");

    assert!(overlay.switch("bars"));
    assert_eq!((windows.opened().len(), window.renderer()), (1, Some(Style::Bars)));
    assert_eq!(window.notified(), [recording(), recording()], "turning on mid-dictation did not resync the current state");
    overlay.close();
    assert!(window.closed());
}

#[test]
fn test_a_window_that_fails_to_open_leaves_the_overlay_off_with_the_error_cue() {
    let cues = Arc::new(FakeCues::default());
    let overlay = active_overlay("fluid", &Windows::failing(), cues.clone());
    assert_eq!(overlay.style(), "off");
    assert!(!overlay.switch("bars"));
    assert_eq!((overlay.style().as_str(), cues.played()), ("off", vec![Cue::Error, Cue::Error]));
    overlay.notify(&recording());
    overlay.close();
}

fn overlay_items(tray: &Tray) -> Vec<MenuItem> {
    let menu = tray.menu();
    let overlays: Vec<_> = menu.iter().filter(|item| item.text == "Overlay").collect();
    assert_eq!(overlays.len(), 1);
    overlays[0].submenu.clone()
}

fn checked(tray: &Tray) -> Vec<bool> {
    overlay_items(tray).iter().map(|item| item.checked).collect()
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn local_server(dir: &Path, port: u16) -> Arc<DictationServer> {
    Arc::new(DictationServer::new(ServerParts {
        config: ServerConfig { bind: "127.0.0.1".to_owned(), port, ..ServerConfig::default() },
        model: Arc::new(SharedModel::default()),
        polisher: None,
        polish_enabled: Arc::new(|| false),
        timeout_s: 1.0,
        token_path: dir.join("server_token.txt"),
    }))
}

struct SilentShell;

impl TrayShell for SilentShell {
    fn show(&self, _glyph: Glyph, _title: &str) {}
    fn notify(&self, _message: &str) {}
    fn stop(&self) {}
}

fn make_tray(dir: &Path, overlay: Arc<ActiveOverlay>, server: Arc<DictationServer>, cues: Arc<FakeCues>) -> Arc<Tray> {
    Tray::new(TrayParts {
        shell: Arc::new(SilentShell),
        choices: Arc::new(TrayChoices::new(&Config::default(), dir.join("tray_choices.json"))),
        overlay,
        polish_available: false,
        server,
        cues,
        postprocess: PostprocessConfig::default(),
        on_settings: Box::new(|| {}),
    })
}

fn saved(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("tray_choices.json")).unwrap()).unwrap()
}

#[test]
fn test_the_overlay_menu_has_a_radio_item_per_style_and_off_that_switches_and_saves() {
    let dir = TempDir::new().unwrap();
    let windows = Windows::default();
    let overlay = Arc::new(active_overlay("off", &windows, Arc::default()));
    let tray = make_tray(dir.path(), overlay.clone(), local_server(dir.path(), 0), Arc::default());
    let items = overlay_items(&tray);
    assert_eq!(items.iter().map(|item| item.text.as_str()).collect::<Vec<_>>(), ["Fluid", "Bars", "Off"]);
    assert!(items.iter().all(|item| item.radio));
    assert_eq!(checked(&tray), [false, false, true]);

    for (item, style) in [(1, "bars"), (0, "fluid"), (2, "off"), (1, "bars")] {
        tray.perform(items[item].action.unwrap());
        assert_eq!(overlay.style(), style);
        let expected: Vec<bool> = items.iter().map(|i| i.text.to_lowercase() == style).collect();
        assert_eq!(checked(&tray), expected);
        assert_eq!(saved(dir.path()), json!({"overlay": style}));
    }
    assert_eq!(windows.opened().len(), 1);
    // pystray names a separator "- - - -"; here it has no text.
    let texts: Vec<_> = tray.menu().iter().map(|item| if item.is_separator() { "- - - -".to_owned() } else { item.text.clone() }).collect();
    assert_eq!(
        texts,
        [
            "Post-process with GLM (no ZAI_API_KEY)",
            "Overlay",
            "Server for Mac",
            "Copy Mac connection info",
            "- - - -",
            "Settings…",
            "Open log",
            "Quit",
        ],
    );
}

fn server_items(tray: &Tray) -> (MenuItem, MenuItem) {
    let item = |text: &str| tray.menu().into_iter().find(|item| item.text == text).unwrap();
    (item("Server for Mac"), item("Copy Mac connection info"))
}

/// One request over a new connection; None when nothing listens on the port.
fn exchange(port: u16, request: &[u8]) -> Option<(u16, Vec<u8>)> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_secs(5)) {
        Ok(stream) => stream,
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => return None,
        Err(e) => panic!("could not connect: {e}"),
    };
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.write_all(request).unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    let head_end = reply.windows(4).position(|window| window == b"\r\n\r\n").expect("a complete reply head");
    let status = std::str::from_utf8(&reply[9..12]).unwrap().parse().unwrap();
    Some((status, reply[head_end + 4..].to_vec()))
}

/// None when nothing listens on the port.
fn health_status(port: u16, token: &str) -> Option<u16> {
    let request = format!(
        "GET {HEALTH_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    exchange(port, request.as_bytes()).map(|reply| reply.0)
}

fn post_wav(port: u16, token: &str, wav: &[u8]) -> Value {
    let boundary = "parakeet-test-boundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut request = format!(
        "POST {TRANSCRIPTIONS_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len(),
    )
    .into_bytes();
    request.extend_from_slice(&body);
    let (status, reply) = exchange(port, &request).expect("the server is listening");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&reply));
    serde_json::from_slice(&reply).unwrap()
}

#[test]
fn test_the_server_item_starts_and_stops_the_server_and_saves_the_choice() {
    let dir = TempDir::new().unwrap();
    let port = free_port();
    let server = local_server(dir.path(), port);
    let overlay = Arc::new(active_overlay("off", &Windows::default(), Arc::default()));
    let tray = make_tray(dir.path(), overlay, server.clone(), Arc::default());
    let states = || {
        let (toggle, copy) = server_items(&tray);
        (toggle.checked, copy.enabled)
    };
    assert_eq!(states(), (false, false));
    let toggle = server_items(&tray).0.action.unwrap();
    for _ in 0..2 {
        tray.perform(toggle);
        assert_eq!(states(), (true, true));
        assert_eq!(saved(dir.path()), json!({"server": true}));
        assert_eq!(health_status(port, &server.token()), Some(200));
        tray.perform(toggle);
        assert_eq!(states(), (false, false));
        assert_eq!(saved(dir.path()), json!({"server": false}));
        assert_eq!(health_status(port, &server.token()), None);
    }
    server.stop();
}

#[test]
fn test_a_port_in_use_leaves_the_server_off_with_the_error_cue() {
    let dir = TempDir::new().unwrap();
    let cues = Arc::new(FakeCues::default());
    let blocker = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = local_server(dir.path(), blocker.local_addr().unwrap().port());
    let overlay = Arc::new(active_overlay("off", &Windows::default(), Arc::default()));
    let tray = make_tray(dir.path(), overlay, server.clone(), cues.clone());
    tray.perform(server_items(&tray).0.action.unwrap());
    drop(blocker);
    let (toggle, copy) = server_items(&tray);
    assert_eq!((server.running(), toggle.checked, copy.enabled, cues.played()), (false, false, false, vec![Cue::Error]));
    assert!(!dir.path().join("tray_choices.json").exists());
}

fn wait_until(condition: impl Fn() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct FakeModel;

impl LoadedModel for FakeModel {
    fn transcribe(&mut self, _audio: &Audio) -> Result<String, String> {
        Ok("from the loaded model".to_owned())
    }
}

struct FakeDictation(Arc<Harness>);

impl RunningDictation for FakeDictation {
    fn finish(&self) {
        self.0.finished.store(true, Ordering::SeqCst);
    }

    fn stop(&self) {}
}

/// Answers every polish with a 500, keeping what was asked.
struct Refusing(Arc<Mutex<Vec<(String, Headers)>>>);

impl Transport for Refusing {
    fn options(&self, _url: &str, _headers: &Headers) -> Result<Reply, TransportError> {
        Ok(Reply { status: 500, body: vec![] })
    }

    fn post(&self, url: &str, headers: &Headers, _body: &[u8]) -> Result<Reply, TransportError> {
        self.0.lock().unwrap().push((url.to_owned(), headers.clone()));
        Ok(Reply { status: 500, body: vec![] })
    }
}

/// A machine of its own for one test: no app mutex, no keyboard hook, nothing reaches a running app.
struct Harness {
    names: Names,
    dir: TempDir,
    config: Config,
    api_key: Option<String>,
    windows: Windows,
    dictations: Mutex<Vec<DictationParts>>,
    myself: Mutex<Option<Arc<Harness>>>,
    requested_keys: Mutex<Vec<String>>,
    polish_requests: Arc<Mutex<Vec<(String, Headers)>>>,
    shown: Mutex<Vec<(Glyph, String)>>,
    notifications: Mutex<Vec<String>>,
    launches: Mutex<Vec<Vec<String>>>,
    finished: AtomicBool,
    stopped: AtomicBool,
}

static NEXT_TAG: AtomicU64 = AtomicU64::new(0);

impl Harness {
    fn new(config: Config, api_key: Option<&str>) -> Arc<Self> {
        let tag = format!("{}-{}", std::process::id(), NEXT_TAG.fetch_add(1, Ordering::SeqCst));
        let harness = Arc::new(Self {
            names: Names {
                mutex: format!("{MUTEX_NAME}-test-{tag}"),
                quit: format!("{QUIT_EVENT_NAME}-test-{tag}"),
                launch: format!("{LAUNCH_EVENT_NAME}-test-{tag}"),
                reload: format!("{RELOAD_EVENT_NAME}-test-{tag}"),
            },
            dir: TempDir::new().unwrap(),
            config,
            api_key: api_key.map(str::to_owned),
            windows: Windows::default(),
            dictations: Mutex::default(),
            myself: Mutex::default(),
            requested_keys: Mutex::default(),
            polish_requests: Arc::default(),
            shown: Mutex::default(),
            notifications: Mutex::default(),
            launches: Mutex::default(),
            finished: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        });
        *harness.myself.lock().unwrap() = Some(harness.clone());
        harness
    }

    fn plain() -> Arc<Self> {
        Self::new(quiet_config(), None)
    }

    fn ready(&self) -> bool {
        !self.dictations.lock().unwrap().is_empty() && self.shown.lock().unwrap().iter().any(|(_, title)| title.ends_with(": ready"))
    }

    fn notifications(&self) -> Vec<String> {
        self.notifications.lock().unwrap().clone()
    }

    fn launches(&self) -> Vec<Vec<String>> {
        self.launches.lock().unwrap().clone()
    }

    fn with_local<R>(&self, read: impl FnOnce(&DictationParts) -> R) -> R {
        read(&self.dictations.lock().unwrap()[0])
    }

    fn dir(&self) -> &Path {
        self.dir.path()
    }
}

fn quiet_config() -> Config {
    Config { overlay: "off".to_owned(), sounds: false, ..Config::default() }
}

impl Machine for Harness {
    fn names(&self) -> Names {
        self.names.clone()
    }

    fn setup_logging(&self) {}

    fn load_config(&self) -> Result<Config, ConfigError> {
        // Whatever a test configures, the server listens on loopback only, on a port of the system's choosing.
        let server = ServerConfig { bind: "127.0.0.1".to_owned(), port: 0, ..self.config.server.clone() };
        Ok(Config { server, ..self.config.clone() })
    }

    fn load_api_key(&self, name: &str) -> Option<String> {
        self.requested_keys.lock().unwrap().push(name.to_owned());
        self.api_key.clone()
    }

    fn polish_transport(&self) -> Option<Box<dyn Transport>> {
        Some(Box::new(Refusing(self.polish_requests.clone())))
    }

    fn tray_choices_path(&self) -> PathBuf {
        self.dir().join("tray_choices.json")
    }

    fn server_token_path(&self) -> PathBuf {
        self.dir().join("server_token.txt")
    }

    fn open_overlay(&self, style: Style, _levels: Box<dyn Fn() -> (Levels, Spectrum) + Send>) -> Result<Box<dyn OverlaySurface>, String> {
        self.windows.open(style)
    }

    fn load_model(&self, _model_dir: &Path, _engine: Engine) -> Result<ReadyModel, String> {
        Ok(ReadyModel { model: Box::new(FakeModel), label: "fake".to_owned(), provider: "fake".to_owned() })
    }

    fn start_dictation(&self, parts: DictationParts) -> Result<Box<dyn RunningDictation>, String> {
        self.dictations.lock().unwrap().push(parts);
        let harness = self.myself.lock().unwrap().clone().expect("the harness is running");
        Ok(Box::new(FakeDictation(harness)))
    }

    fn launch(&self, arguments: &[&str]) -> std::io::Result<()> {
        self.launches.lock().unwrap().push(arguments.iter().map(|argument| (*argument).to_owned()).collect());
        Ok(())
    }
}

type During = Box<dyn FnOnce(&Arc<Tray>) + Send>;

struct FakeTrayLoop {
    harness: Arc<Harness>,
    during: Mutex<Option<During>>,
}

impl FakeTrayLoop {
    fn new(harness: &Arc<Harness>, during: During) -> Arc<Self> {
        Arc::new(Self { harness: harness.clone(), during: Mutex::new(Some(during)) })
    }
}

impl TrayShell for FakeTrayLoop {
    fn show(&self, glyph: Glyph, title: &str) {
        self.harness.shown.lock().unwrap().push((glyph, title.to_owned()));
    }

    fn notify(&self, message: &str) {
        self.harness.notifications.lock().unwrap().push(message.to_owned());
    }

    fn stop(&self) {
        self.harness.stopped.store(true, Ordering::SeqCst);
    }
}

impl TrayLoop for FakeTrayLoop {
    fn run_until_stopped(&self, tray: Arc<Tray>) {
        wait_until(|| self.harness.ready(), "the loader never got ready");
        let during = self.during.lock().unwrap().take().expect("the tray loop runs once");
        during(&tray);
    }
}

/// Run the app under `harness` with these command-line arguments, doing `during` inside the tray loop
/// once the model is ready.
fn run(harness: &Arc<Harness>, arguments: &[&str], during: impl FnOnce(&Arc<Tray>) + Send + 'static) -> i32 {
    let arguments = parse_arguments(arguments.iter().map(|argument| (*argument).to_owned())).unwrap();
    let code = run_app(arguments, harness.clone(), FakeTrayLoop::new(harness, Box::new(during)));
    // What the app handed over keeps the harness alive through it; release it so the temp directory goes.
    harness.dictations.lock().unwrap().clear();
    *harness.myself.lock().unwrap() = None;
    code
}

// Dropped from the Python test, since nothing here installs a logger: the "ready: ... overlay off" and
// "overlay switched to bars from the tray" log lines.
#[test]
fn test_main_wires_the_tray_menu_to_the_overlay_and_the_dictation() {
    let harness = Harness::plain();
    let inside = harness.clone();
    let during = move |tray: &Arc<Tray>| {
        inside.with_local(|local| {
            (local.on_transition)(&recording());
            assert_eq!(inside.windows.opened().len(), 0, "opened while off");
            let items = overlay_items(tray);
            assert_eq!(items.iter().map(|item| item.text.as_str()).collect::<Vec<_>>(), ["Fluid", "Bars", "Off"]);
            tray.perform(items.iter().find(|item| item.text == "Bars").unwrap().action.unwrap());
            assert_eq!(checked(tray), [false, true, false]);
            (local.on_transition)(&recording());
        });
    };
    assert_eq!(run(&harness, &[], during), 0);

    let opened = harness.windows.opened();
    assert_eq!(opened.len(), 1);
    let window = &opened[0];
    assert_eq!((window.renderer(), window.notified(), window.closed()), (Some(Style::Bars), vec![recording(), recording()], true));
    assert_eq!(saved(harness.dir()), json!({"overlay": "bars"}));
}

// Dropped from the Python test: that the HTTP clients are closed at the end. A Rust `Polisher` has
// nothing to close; dropping it releases its connections.
#[test]
fn test_main_selects_claude_for_local_and_server_dictations_and_names_its_key() {
    for available in [true, false] {
        let config = Config { postprocess: PostprocessConfig::for_provider(PolishProvider::Anthropic), ..quiet_config() };
        let harness = Harness::new(config, available.then_some("test-key"));
        let inside = harness.clone();
        let during = move |tray: &Arc<Tray>| {
            let item = tray.menu().swap_remove(0);
            let suffix = if available { "" } else { " (no ANTHROPIC_API_KEY)" };
            assert_eq!(item.text, format!("Post-process with Claude{suffix}"));
            assert_eq!((item.enabled, item.checked), (available, available));
            let server = tray.server().clone();
            inside.with_local(|local| match (&local.polisher, server.polisher()) {
                (Some(polisher), Some(shared)) => {
                    assert!(available);
                    assert!(std::ptr::addr_eq(Arc::as_ptr(polisher), Arc::as_ptr(shared)));
                    // A Claude client: it posts to Anthropic's endpoint with the x-api-key header.
                    assert!(polisher.polish("raw", "").is_err());
                    let requests = inside.polish_requests.lock().unwrap();
                    let (url, headers) = &requests[0];
                    assert_eq!(*url, PostprocessConfig::for_provider(PolishProvider::Anthropic).endpoint);
                    assert!(headers.contains(&("x-api-key".to_owned(), "test-key".to_owned())));
                    tray.perform(item.action.unwrap());
                    assert!(!(local.polish_enabled)() && !server.polish_on());
                }
                (None, None) => assert!(!available),
                _ => panic!("local dictation and the server hold different polishers"),
            });
        };
        assert_eq!(run(&harness, &[], during), 0);
        assert_eq!(*harness.requested_keys.lock().unwrap(), ["ANTHROPIC_API_KEY"]);
    }
}

#[test]
fn test_settings_is_the_default_tray_action_and_opens_the_window() {
    let harness = Harness::plain();
    let during = |tray: &Arc<Tray>| {
        let item = tray.menu().into_iter().find(|item| item.text == "Settings…").unwrap();
        assert!(item.default);
        tray.perform(item.action.unwrap());
    };
    assert_eq!(run(&harness, &[], during), 0);
    assert_eq!(harness.launches(), [["--settings"]]);
}

#[test]
fn test_saved_settings_finish_dictation_then_restart_the_app() {
    let harness = Harness::plain();
    let inside = harness.clone();
    let during = move |_tray: &Arc<Tray>| {
        assert!(signal_event(&inside.names.reload));
        wait_until(|| inside.finished.load(Ordering::SeqCst), "the dictation was never finished");
        wait_until(|| inside.stopped.load(Ordering::SeqCst), "the tray was never stopped");
    };
    assert_eq!(run(&harness, &[], during), 0);
    assert_eq!(harness.launches(), [["--notify-ready"]]);
}

// The tray window itself titles every notification "Parakeet Dictate", so `TrayShell::notify` takes
// only the message, and only the message is compared here and below.
#[test]
fn test_a_second_launch_exits_and_the_running_app_says_it_is_already_running() {
    let harness = Harness::plain();
    let inside = harness.clone();
    let during = move |_tray: &Arc<Tray>| {
        let second = FakeTrayLoop::new(&inside, Box::new(|_| panic!("a second launch ran the tray")));
        assert_eq!(run_app(Default::default(), inside.clone(), second), 0);
        wait_until(|| !inside.notifications().is_empty(), "the running app never showed a notification");
    };
    assert_eq!(run(&harness, &[], during), 0);
    assert_eq!(harness.notifications(), ["Parakeet Dictate is already running. Hold Right Ctrl to dictate."]);
}

#[test]
fn test_notify_ready_shows_one_notification_naming_the_configured_hotkey() {
    let harness = Harness::new(Config { hotkey: "shift_r".to_owned(), ..quiet_config() }, None);
    assert_eq!(run(&harness, &["--notify-ready"], |_| {}), 0);
    assert_eq!(harness.notifications(), ["Ready. Hold Right Shift to dictate."]);
}

#[test]
fn test_a_start_without_notify_ready_shows_no_notification() {
    let harness = Harness::plain();
    assert_eq!(run(&harness, &[], |_| {}), 0);
    assert!(harness.notifications().is_empty());
}

// The port comes from `DictationServer::port` instead of the "server for Mac listening" log line.
#[test]
fn test_a_saved_server_choice_serves_the_loaded_model_until_quit() {
    let harness = Harness::plain();
    std::fs::write(harness.dir().join("tray_choices.json"), r#"{"server": true}"#).unwrap();
    let inside = harness.clone();
    let port = Arc::new(AtomicU64::new(0));
    let seen_port = port.clone();
    let during = move |tray: &Arc<Tray>| {
        let port = tray.server().port().expect("the server is listening");
        seen_port.store(port.into(), Ordering::SeqCst);
        let token = std::fs::read_to_string(inside.dir().join("server_token.txt")).unwrap();
        let reply = post_wav(port, &token, &second_of_silence());
        assert_eq!(reply, json!({"text": "from the loaded model", "raw": "from the loaded model", "polished": false}));
        let audio = Audio { samples: vec![], rate: 16_000 };
        let shared = inside.with_local(|local| local.transcriber.transcribe(&audio));
        assert_eq!(shared.unwrap(), "from the loaded model", "dictation shares the model");
        assert!(server_items(tray).0.checked);
    };
    assert_eq!(run(&harness, &[], during), 0);
    let port = port.load(Ordering::SeqCst) as u16;
    assert_eq!(health_status(port, "any"), None, "the server outlived quit");
}

const COLOR: u32 = 0x12_34_56;

fn pixel(pixels: &[u8], size: usize, row: usize, column: usize) -> [u8; 4] {
    pixels[(row * size + column) * 4..][..4].try_into().unwrap()
}

fn assert_transparent_corners(pixels: &[u8], size: usize) {
    for (row, column) in [(0, 0), (0, size - 1), (size - 1, 0), (size - 1, size - 1)] {
        assert_eq!(pixel(pixels, size, row, column)[3], 0, "corner {row},{column} at size {size}");
    }
}

#[test]
fn test_a_disc_is_opaque_at_its_center_with_transparent_corners() {
    for size in [16, 20, 32, 64] {
        let pixels = glyph_pixels(Glyph::Disc(COLOR), size);
        assert_eq!(pixels.len(), size * size * 4);
        assert_transparent_corners(&pixels, size);
        assert_eq!(pixel(&pixels, size, size / 2, size / 2), [0x56, 0x34, 0x12, 255]);
    }
}

#[test]
fn test_a_ring_is_opaque_on_its_band_with_a_transparent_hole_and_corners() {
    for size in [32, 64] {
        let pixels = glyph_pixels(Glyph::Ring(COLOR), size);
        assert_eq!(pixels.len(), size * size * 4);
        assert_transparent_corners(&pixels, size);
        assert_eq!(pixel(&pixels, size, size / 2, size / 2)[3], 0, "the hole at size {size}");
        // Radius 20 of 64: halfway between the hole's 16 and the rim's 24.
        let band = size / 2 - size * 20 / 64;
        assert_eq!(pixel(&pixels, size, size / 2, band), [0x56, 0x34, 0x12, 255], "the band at size {size}");
    }
}
