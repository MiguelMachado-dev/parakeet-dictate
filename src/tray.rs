//! The tray icon's menu and what it shows. `TrayShell` is the part that talks to Windows.

use std::sync::{Arc, Mutex};
use std::thread;

use log::{error, info};
use serde_json::json;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::config::{OVERLAY_CHOICES, PostprocessConfig};
use crate::cues::PlayCue;
use crate::inject;
use crate::overlay::active::ActiveOverlay;
use crate::paths;
use crate::reducer::{Cue, State};
use crate::server::DictationServer;
use crate::tray_choices::TrayChoices;
use crate::win::{wide, wide_path};

const IDLE: u32 = 0x9e9e9e;
const RECORDING: u32 = 0xe53935;
const TRANSCRIBING: u32 = 0xffb300;
const POLISHING: u32 = 0x8e24aa;
const TITLE_LIMIT: usize = 120;

/// The icon: a filled disc while the app works, a ring while it loads or after it failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    Disc(u32),
    Ring(u32),
}

pub trait TrayShell: Send + Sync {
    fn show(&self, glyph: Glyph, title: &str);
    fn notify(&self, message: &str);
    fn stop(&self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Postprocess,
    Overlay(&'static str),
    Server,
    CopyConnectionInfo,
    Settings,
    OpenLog,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MenuItem {
    pub text: String,
    pub action: Option<Action>,
    pub checked: bool,
    pub enabled: bool,
    pub radio: bool,
    pub default: bool,
    pub submenu: Vec<MenuItem>,
}

impl MenuItem {
    fn new(text: &str, action: Action) -> Self {
        Self { text: text.to_owned(), action: Some(action), enabled: true, ..Self::default() }
    }

    fn separator() -> Self {
        Self::default()
    }

    pub fn is_separator(&self) -> bool {
        self.text.is_empty()
    }
}

pub struct TrayParts {
    pub shell: Arc<dyn TrayShell>,
    pub choices: Arc<TrayChoices>,
    pub overlay: Arc<ActiveOverlay>,
    pub polish_available: bool,
    pub server: Arc<DictationServer>,
    pub cues: Arc<dyn PlayCue>,
    pub postprocess: PostprocessConfig,
    pub on_settings: Box<dyn Fn() + Send + Sync>,
}

pub struct Tray {
    parts: TrayParts,
    provider: Mutex<String>,
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

impl Tray {
    pub fn new(parts: TrayParts) -> Arc<Self> {
        parts.shell.show(Glyph::Ring(IDLE), "Parakeet Dictate: loading model");
        Arc::new(Self { parts, provider: Mutex::new(String::new()) })
    }

    pub fn server(&self) -> &Arc<DictationServer> {
        &self.parts.server
    }

    pub fn set_provider(&self, label: &str) {
        label.clone_into(&mut self.provider.lock().unwrap());
    }

    /// Asked anew each time the menu opens, so it always shows the current state.
    pub fn menu(&self) -> Vec<MenuItem> {
        let parts = &self.parts;
        let available = parts.polish_available;
        let suffix = if available { String::new() } else { format!(" (no {})", parts.postprocess.api_key_name()) };
        let postprocess = MenuItem {
            checked: available && parts.choices.postprocess(),
            enabled: available,
            ..MenuItem::new(&format!("Post-process with {}{suffix}", parts.postprocess.label()), Action::Postprocess)
        };
        let style = parts.overlay.style();
        let styles = OVERLAY_CHOICES.iter().map(|choice| MenuItem {
            checked: style == *choice,
            radio: true,
            ..MenuItem::new(&capitalized(choice), Action::Overlay(choice))
        });
        let overlay = MenuItem {
            text: "Overlay".to_owned(),
            enabled: true,
            submenu: styles.collect(),
            ..MenuItem::default()
        };
        let running = parts.server.running();
        vec![
            postprocess,
            overlay,
            MenuItem { checked: running, ..MenuItem::new("Server for Mac", Action::Server) },
            MenuItem { enabled: running, ..MenuItem::new("Copy Mac connection info", Action::CopyConnectionInfo) },
            MenuItem::separator(),
            MenuItem { default: true, ..MenuItem::new("Settings…", Action::Settings) },
            MenuItem::new("Open log", Action::OpenLog),
            MenuItem::new("Quit", Action::Quit),
        ]
    }

    pub fn perform(self: &Arc<Self>, action: Action) {
        match action {
            Action::Postprocess => self.toggle_postprocess(),
            Action::Overlay(style) => self.switch_overlay(style),
            Action::Server => self.toggle_server(),
            Action::CopyConnectionInfo => {
                // Its own thread: finding the LAN address takes PowerShell a second or two.
                let tray = self.clone();
                let copied = thread::Builder::new().name("connection-info".to_owned()).spawn(move || tray.copy_connection_info());
                if let Err(e) = copied {
                    error!("could not start copying the connection info: {e}");
                }
            }
            Action::Settings => (self.parts.on_settings)(),
            Action::OpenLog => open_log(),
            Action::Quit => self.parts.shell.stop(),
        }
    }

    fn switch_overlay(&self, style: &str) {
        if self.parts.overlay.switch(style) {
            info!("overlay switched to {style} from the tray");
            self.parts.choices.choose("overlay", json!(style));
        }
    }

    fn toggle_postprocess(&self) {
        let on = !self.parts.choices.postprocess();
        info!("post-processing turned {} from the tray", if on { "on" } else { "off" });
        self.parts.choices.choose("postprocess", json!(on));
    }

    pub fn start_server(&self) -> bool {
        match self.parts.server.start() {
            Ok(()) => true,
            Err(e) => {
                error!("could not start the server for Mac: {e}");
                self.parts.cues.play(Cue::Error);
                false
            }
        }
    }

    fn toggle_server(&self) {
        if self.parts.server.running() {
            self.parts.server.stop();
            self.parts.choices.choose("server", json!(false));
            info!("server for Mac turned off from the tray");
        } else if self.start_server() {
            self.parts.choices.choose("server", json!(true));
            info!("server for Mac turned on from the tray");
        }
    }

    fn copy_connection_info(&self) {
        let Some(connection) = self.parts.server.connection_info() else {
            self.parts.cues.play(Cue::Error);
            self.notify("Could not find this PC's address on the local network.");
            return;
        };
        if let Err(e) = inject::set_text(Some(&connection), true) {
            error!("could not copy the Mac connection info: {e}");
            self.parts.cues.play(Cue::Error);
            return;
        }
        let url = connection.lines().next().unwrap_or_default();
        info!("copied the Mac connection info: {url} and the token");
        self.notify(&format!("Copied {url} and the token. Paste them into the Mac app."));
    }

    pub fn show_state(&self, state: &State) {
        let (color, status) = match state {
            State::Recording { .. } => (RECORDING, "recording"),
            State::Transcribing { .. } | State::AwaitingRelease { .. } => (TRANSCRIBING, "transcribing"),
            State::Polishing { .. } => (POLISHING, "polishing"),
            State::Idle { .. } => (IDLE, "ready"),
        };
        let provider = self.provider.lock().unwrap();
        self.parts.shell.show(Glyph::Disc(color), &format!("Parakeet Dictate ({provider}): {status}"));
    }

    pub fn show_error(&self, message: &str) {
        let title: String = format!("Parakeet Dictate failed: {message}").chars().take(TITLE_LIMIT).collect();
        self.parts.shell.show(Glyph::Ring(RECORDING), &title);
    }

    pub fn notify(&self, message: &str) {
        self.parts.shell.notify(message);
    }

    pub fn stop(&self) {
        self.parts.shell.stop();
    }
}

fn open_log() {
    let (open, log) = (wide("open"), wide_path(&paths::log()));
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), open.as_ptr(), log.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}
