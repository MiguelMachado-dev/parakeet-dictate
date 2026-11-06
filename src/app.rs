//! The tray app: startup, the threads that watch the other processes, and shutdown.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use log::{error, info};
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};

use crate::config::{self, Config, ConfigError, Engine, PolishProvider, PostprocessConfig};
use crate::cues::{Cues, PlayCue};
use crate::dictation::{Dictation, DictationParts};
use crate::instance::{self, Handle, LAUNCH_EVENT_NAME, MUTEX_NAME, QUIT_EVENT_NAME, RELOAD_EVENT_NAME, create_event, signal_event};
use crate::logging;
use crate::meter::{Levels, Spectrum};
use crate::overlay::active::ActiveOverlay;
use crate::overlay::window::{Overlay, OverlaySurface};
use crate::overlay::Style;
use crate::paths;
use crate::polish::{Polish, Transport, create_polisher};
use crate::recorder::{self, Microphone};
use crate::reducer::{Cue, IDLE, Transition};
use crate::server::{DictationServer, ServerParts};
use crate::settings;
use crate::transcriber::{LoadedModel, SharedModel, Transcribe, Transcriber};
use crate::tray::{Tray, TrayParts, TrayShell};
use crate::tray_choices::TrayChoices;
use crate::tray_window::TrayWindow;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Arguments {
    pub quit: bool,
    pub notify_ready: bool,
    pub settings: bool,
}

pub fn parse_arguments(arguments: impl Iterator<Item = String>) -> Result<Arguments, String> {
    let mut parsed = Arguments::default();
    for argument in arguments {
        match argument.as_str() {
            "--quit" => parsed.quit = true,
            "--notify-ready" => parsed.notify_ready = true,
            "--settings" => parsed.settings = true,
            other => return Err(format!("unrecognized argument: {other}")),
        }
    }
    Ok(parsed)
}

/// The kernel object names of one running app.
#[derive(Clone, Debug)]
pub struct Names {
    pub mutex: String,
    pub quit: String,
    pub launch: String,
    pub reload: String,
}

impl Default for Names {
    fn default() -> Self {
        Self {
            mutex: MUTEX_NAME.to_owned(),
            quit: QUIT_EVENT_NAME.to_owned(),
            launch: LAUNCH_EVENT_NAME.to_owned(),
            reload: RELOAD_EVENT_NAME.to_owned(),
        }
    }
}

pub struct ReadyModel {
    pub model: Box<dyn LoadedModel>,
    pub label: String,
    pub provider: String,
}

/// The dictation the app runs, stopped from the tray's thread while a reload may still wait on it.
pub trait RunningDictation: Send + Sync {
    fn finish(&self);
    fn stop(&self);
}

impl RunningDictation for Dictation {
    fn finish(&self) {
        Self::finish(self);
    }

    fn stop(&self) {
        Self::stop(self);
    }
}

/// What `run` takes from the machine. Tests replace it, so they share no mutex, event, or file with a running app.
pub trait Machine: Send + Sync {
    fn names(&self) -> Names {
        Names::default()
    }

    fn setup_logging(&self) {
        logging::setup();
    }

    fn load_config(&self) -> Result<Config, ConfigError> {
        config::load_config(&paths::config())
    }

    fn load_api_key(&self, name: &str) -> Option<String> {
        config::load_api_key(&paths::env_file(), name)
    }

    fn polish_transport(&self) -> Option<Box<dyn Transport>> {
        None
    }

    fn tray_choices_path(&self) -> PathBuf {
        paths::tray_choices()
    }

    fn server_token_path(&self) -> PathBuf {
        paths::server_token()
    }

    fn open_overlay(&self, style: Style, levels: Box<dyn Fn() -> (Levels, Spectrum) + Send>) -> Result<Box<dyn OverlaySurface>, String> {
        Ok(Box::new(Overlay::new(style, levels)?))
    }

    fn load_model(&self, model_dir: &std::path::Path, engine: Engine) -> Result<ReadyModel, String> {
        let mut transcriber = Transcriber::load(model_dir, engine)?;
        transcriber.warm_up(recorder::default_input_rate())?;
        let (label, provider) = (transcriber.label().to_owned(), transcriber.engine().name().to_owned());
        Ok(ReadyModel { model: Box::new(transcriber), label, provider })
    }

    fn start_dictation(&self, parts: DictationParts) -> Result<Box<dyn RunningDictation>, String> {
        let dictation = Dictation::new(parts);
        dictation.listen()?;
        Ok(Box::new(dictation))
    }

    fn launch(&self, arguments: &[&str]) -> std::io::Result<()> {
        instance::launch_background(arguments)
    }
}

pub struct RealMachine;

impl Machine for RealMachine {}

/// The tray's message loop, or what stands in for it in a test.
pub trait TrayLoop: TrayShell {
    fn run_until_stopped(&self, tray: Arc<Tray>);
}

impl TrayLoop for TrayWindow {
    fn run_until_stopped(&self, tray: Arc<Tray>) {
        self.run(tray);
    }
}

fn postprocessing_summary(config: &PostprocessConfig, available: bool, on: bool) -> String {
    if !available {
        return format!("unavailable, no {} in the environment or .env", config.api_key_name());
    }
    let anthropic = config.provider == PolishProvider::Anthropic;
    let effort = if anthropic { &config.effort } else { &config.reasoning_effort };
    let mut settings = format!("{}, effort {effort}", config.model);
    if anthropic {
        settings.push_str(&format!(", thinking {}", if config.thinking { "on" } else { "off" }));
    }
    format!("{} ({settings})", if on { "on" } else { "off" })
}

/// Everything the threads share once the app is up.
struct Running {
    machine: Arc<dyn Machine>,
    arguments: Arguments,
    config: Config,
    hint: String,
    choices: Arc<TrayChoices>,
    cues: Arc<dyn PlayCue>,
    overlay: Arc<ActiveOverlay>,
    model: Arc<SharedModel>,
    server: Arc<DictationServer>,
    polisher: Option<Arc<dyn Polish>>,
    microphone: Mutex<Option<Microphone>>,
    tray: Arc<Tray>,
    stopping: AtomicBool,
    restarting: AtomicBool,
    loader: Mutex<Option<JoinHandle<()>>>,
    dictation: Mutex<Option<Arc<dyn RunningDictation>>>,
}

impl Running {
    fn load(self: &Arc<Self>) {
        if let Err(e) = self.try_load() {
            error!("startup failed: {e}");
            self.tray.show_error(&e);
            self.cues.play(Cue::Error);
        }
    }

    fn try_load(self: &Arc<Self>) -> Result<(), String> {
        let ready = self.machine.load_model(&self.config.model_dir, self.config.provider)?;
        if self.stopping.load(Ordering::Acquire) {
            return Ok(());
        }
        self.model.load(ready.model);
        self.tray.set_provider(&ready.label);

        let (tray, overlay) = (self.tray.clone(), self.overlay.clone());
        let on_transition = Box::new(move |transition: &Transition| {
            tray.show_state(&transition.state);
            overlay.notify(transition);
        });
        let choices = self.choices.clone();
        let parts = DictationParts::new(
            self.config.clone(),
            Box::new(self.microphone.lock().unwrap().take().expect("the loader runs once")),
            self.model.clone() as Arc<dyn Transcribe>,
            self.cues.clone(),
            on_transition,
            self.polisher.clone(),
            Arc::new(move || choices.postprocess()),
        );
        let dictation: Arc<dyn RunningDictation> = Arc::from(self.machine.start_dictation(parts)?);
        *self.dictation.lock().unwrap() = Some(dictation);
        self.tray.show_state(&IDLE);
        let postprocess = &self.config.postprocess;
        info!(
            "ready: provider {}, hotkey {}, post-processing {}, server for Mac {}, overlay {}",
            ready.provider,
            self.config.hotkey,
            postprocessing_summary(postprocess, self.polisher.is_some(), self.choices.postprocess()),
            self.server.port().map_or_else(|| "off".to_owned(), |port| format!("on port {port}")),
            self.overlay.style(),
        );
        if self.arguments.notify_ready {
            self.tray.notify(&format!("Ready. {}", self.hint));
        }
        Ok(())
    }

    fn join_loader(&self) {
        let mut loader = self.loader.lock().unwrap();
        if let Some(loader) = loader.take() {
            let _ = loader.join();
        }
    }

    fn current_dictation(&self) -> Option<Arc<dyn RunningDictation>> {
        self.dictation.lock().unwrap().clone()
    }

    fn reload_when_idle(&self) {
        info!("settings saved; finishing the current dictation before reloading");
        self.join_loader();
        if let Some(dictation) = self.current_dictation() {
            dictation.finish();
        }
        if !self.stopping.load(Ordering::Acquire) {
            self.restarting.store(true, Ordering::Release);
            self.tray.stop();
        }
    }

    fn watch(self: &Arc<Self>, quit: &Handle, launch: &Handle, reload: &Handle, shutdown: &Handle) {
        let handles: [HANDLE; 4] = [quit.0, launch.0, reload.0, shutdown.0];
        let mut reloading = false;
        loop {
            let signalled = unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, INFINITE) };
            match signalled.wrapping_sub(WAIT_OBJECT_0) {
                0 => {
                    info!("quit requested by another process");
                    self.tray.stop();
                    return;
                }
                1 => {
                    info!("launched again while running");
                    self.tray.notify(&format!("Parakeet Dictate is already running. {}", self.hint));
                }
                2 if !reloading => {
                    reloading = true;
                    // On its own thread: the wait can last a whole dictation, and a quit must still get through.
                    let running = self.clone();
                    let spawned = thread::Builder::new().name("settings-reload".to_owned()).spawn(move || running.reload_when_idle());
                    if let Err(e) = spawned {
                        error!("could not start the reload: {e}");
                    }
                }
                2 => {}
                _ => return,
            }
        }
    }
}

fn unnamed_event() -> Result<Handle, String> {
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if handle.is_null() {
        return Err(crate::win::last_error("CreateEventW"));
    }
    Ok(Handle(handle))
}

fn start_app(arguments: Arguments, machine: Arc<dyn Machine>, shell: Arc<dyn TrayLoop>) -> Result<i32, String> {
    let names = machine.names();
    let Some(mutex) = instance::acquire_named(&names.mutex)? else {
        let told = signal_event(&names.launch);
        info!("another instance is already running; {}", if told { "asked it to say so" } else { "it is still starting" });
        return Ok(0);
    };
    info!("starting, pid {}", std::process::id());
    // Publish before reading settings, so a save during startup can request another reload.
    let reload = create_event(&names.reload, false)?;
    let config = match machine.load_config() {
        Ok(config) => config,
        Err(e) => {
            error!("invalid config: {e}");
            return Ok(1);
        }
    };
    let quit = create_event(&names.quit, true)?;
    // Auto-reset, so each later launch wakes the watcher once.
    let launch = create_event(&names.launch, false)?;
    let shutdown = unnamed_event()?;

    let hint = format!("Hold {} to dictate.", config.hotkey_label());
    let api_key = machine.load_api_key(config.postprocess.api_key_name());
    let polisher: Option<Arc<dyn Polish>> = api_key.map(|key| {
        let transport = machine.polish_transport();
        Arc::new(create_polisher(&config.postprocess, &key, transport)) as Arc<dyn Polish>
    });
    let choices = Arc::new(TrayChoices::new(&config, machine.tray_choices_path()));
    let cues: Arc<dyn PlayCue> = Arc::new(Cues::new(config.sounds));
    let microphone = Microphone::new();
    let meter = microphone.meter();
    let opener = machine.clone();
    let open_window = Box::new(move |style: Style| {
        let meter = meter.clone();
        opener.open_overlay(style, Box::new(move || meter.get().snapshot()))
    });
    let overlay = Arc::new(ActiveOverlay::new(&choices.overlay(), open_window, cues.clone()));
    let model = Arc::new(SharedModel::default());
    let polish_choices = choices.clone();
    let server = Arc::new(DictationServer::new(ServerParts {
        config: config.server.clone(),
        model: model.clone(),
        polisher: polisher.clone(),
        polish_enabled: Arc::new(move || polish_choices.postprocess()),
        timeout_s: config.postprocess.timeout_s,
        token_path: machine.server_token_path(),
    }));
    let launcher = machine.clone();
    let tray = Tray::new(TrayParts {
        shell: shell.clone(),
        choices: choices.clone(),
        overlay: overlay.clone(),
        polish_available: polisher.is_some(),
        server: server.clone(),
        cues: cues.clone(),
        postprocess: config.postprocess.clone(),
        on_settings: Box::new(move || {
            if let Err(e) = launcher.launch(&["--settings"]) {
                error!("could not open Settings: {e}");
            }
        }),
    });
    // Before the model loads, so a Mac sees 503 "still loading" instead of no server.
    if choices.server() {
        tray.start_server();
    }

    let running = Arc::new(Running {
        machine: machine.clone(),
        arguments,
        config,
        hint,
        choices,
        cues,
        overlay,
        model,
        server,
        polisher,
        microphone: Mutex::new(Some(microphone)),
        tray: tray.clone(),
        stopping: AtomicBool::new(false),
        restarting: AtomicBool::new(false),
        loader: Mutex::new(None),
        dictation: Mutex::new(None),
    });
    let loading = running.clone();
    let loader = thread::Builder::new().name("loader".to_owned()).spawn(move || loading.load()).map_err(|e| e.to_string())?;
    *running.loader.lock().unwrap() = Some(loader);
    let watching = running.clone();
    let shutdown = Arc::new(shutdown);
    let watch_shutdown = shutdown.clone();
    let watcher = thread::Builder::new()
        .name("watcher".to_owned())
        .spawn(move || watching.watch(&quit, &launch, &reload, &watch_shutdown))
        .map_err(|e| e.to_string())?;

    shell.run_until_stopped(tray);

    info!("shutting down");
    running.stopping.store(true, Ordering::Release);
    running.join_loader();
    running.server.stop();
    if let Some(dictation) = running.current_dictation() {
        dictation.stop();
    }
    running.overlay.close();
    unsafe { SetEvent(shutdown.0) };
    let _ = watcher.join();
    drop(mutex);
    info!("stopped");
    if running.restarting.load(Ordering::Acquire)
        && let Err(e) = machine.launch(&["--notify-ready"])
    {
        error!("could not restart: {e}");
    }
    Ok(0)
}

/// The tray app under `machine`, until it quits; the process exit code.
pub fn run_app(arguments: Arguments, machine: Arc<dyn Machine>, shell: Arc<dyn TrayLoop>) -> i32 {
    match start_app(arguments, machine, shell) {
        Ok(code) => code,
        Err(e) => {
            error!("could not start: {e}");
            1
        }
    }
}

pub fn main() -> i32 {
    let arguments = match parse_arguments(std::env::args().skip(1)) {
        Ok(arguments) => arguments,
        Err(e) => {
            logging::setup();
            error!("{e}");
            return 2;
        }
    };
    if arguments.settings {
        return settings::main();
    }
    let machine: Arc<dyn Machine> = Arc::new(RealMachine);
    machine.setup_logging();
    if arguments.quit {
        let stopped = signal_event(&machine.names().quit);
        info!("--quit: {}", if stopped { "signalled the running instance" } else { "no instance running" });
        return i32::from(!stopped);
    }
    let shell = match TrayWindow::new() {
        Ok(shell) => shell,
        Err(e) => {
            error!("could not create the tray window: {e}");
            return 1;
        }
    };
    run_app(arguments, machine, Arc::new(shell))
}
