//! The controller: the one thread that advances the dictation state machine and carries out its effects.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use log::{error, info, warn};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use crate::app_rules::combined_instructions;
use crate::clock;
use crate::config::Config;
use crate::cues::PlayCue;
use crate::foreground::foreground_executable;
use crate::inject;
use crate::keyboard::{KeyEvent, KeyboardHook};
use crate::polish::Polish;
use crate::recorder::{Audio, Record};
use crate::reducer::{Cue, Effect, Event, IDLE, State, Transition, step};
use crate::text::short_float;
use crate::transcriber::Transcribe;

const RELEASE_POLL: Duration = Duration::from_millis(50);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const VK_ESCAPE: u32 = inject::VK_ESCAPE;

enum Message {
    Event(Event),
    /// Stop accepting recordings once the current dictation and hotkey release finish.
    Finish,
    Stop,
}

type OnTransition = Box<dyn Fn(&Transition) + Send>;
type Paste = Box<dyn Fn(&str) -> Result<(), String> + Send>;

pub struct DictationParts {
    pub config: Config,
    pub recorder: Box<dyn Record>,
    pub transcriber: Arc<dyn Transcribe>,
    pub paste: Paste,
    pub cues: Arc<dyn PlayCue>,
    pub on_transition: OnTransition,
    pub polisher: Option<Arc<dyn Polish>>,
    pub polish_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    pub foreground_app: Box<dyn Fn() -> Option<String> + Send>,
    /// Whether the hotkey is physically down, for a release the hook never saw.
    pub hotkey_down: Box<dyn Fn() -> bool + Send>,
}

impl DictationParts {
    /// The real paste, foreground lookup, and key state around what the app supplies.
    pub fn new(
        config: Config,
        recorder: Box<dyn Record>,
        transcriber: Arc<dyn Transcribe>,
        cues: Arc<dyn PlayCue>,
        on_transition: OnTransition,
        polisher: Option<Arc<dyn Polish>>,
        polish_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        let vk = config.hotkey_vk() as i32;
        Self {
            config,
            recorder,
            transcriber,
            paste: Box::new(inject::paste),
            cues,
            on_transition,
            polisher,
            polish_enabled,
            foreground_app: Box::new(foreground_executable),
            hotkey_down: Box::new(move || unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0),
        }
    }
}

/// What the keyboard hook decides. It runs inside the low-level hook: it posts and returns.
pub struct KeyFilter {
    hotkey_vk: u32,
    events: Sender<Message>,
    polishing: Arc<AtomicBool>,
    hotkey_held: bool,
    esc_swallowed: bool,
}

impl KeyFilter {
    /// True swallows the key; otherwise it still reaches the focused app.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        if key.tag == inject::INJECTED_TAG {
            return false;
        }
        let post = |event| {
            let _ = self.events.send(Message::Event(event));
        };
        if key.vk == self.hotkey_vk {
            self.hotkey_held = key.down;
            post(if key.down { Event::HotkeyDown(clock::now()) } else { Event::HotkeyUp(clock::now()) });
        } else if key.vk == VK_ESCAPE {
            if key.down {
                // With Ctrl as the hotkey, Esc while holding it is Ctrl+Esc, which opens Start.
                // While polishing, Esc pastes the raw transcript into the focused app, which
                // must not also act on the Esc.
                self.esc_swallowed = self.hotkey_held || self.polishing.load(Ordering::Acquire);
                post(Event::Cancel);
            }
            return self.esc_swallowed;
        } else if key.down {
            post(Event::OtherKeyDown);
        }
        false
    }
}

pub struct Dictation {
    hotkey_vk: u32,
    events: Sender<Message>,
    polishing: Arc<AtomicBool>,
    controller: Mutex<Option<JoinHandle<()>>>,
    ended: Arc<Ended>,
    hook: Mutex<Option<KeyboardHook>>,
}

/// Set when the controller returns. `finish` and `stop` wait on it from different threads, so a quit
/// while a reload waits for the dictation to end still gets through.
#[derive(Default)]
struct Ended {
    done: Mutex<bool>,
    changed: Condvar,
}

impl Ended {
    fn set(&self) {
        *self.done.lock().unwrap() = true;
        self.changed.notify_all();
    }

    fn wait(&self, limit: Duration) -> bool {
        let done = self.done.lock().unwrap();
        !self.changed.wait_timeout_while(done, limit, |done| !*done).unwrap().1.timed_out()
    }
}

impl Dictation {
    /// Start the controller. Keys reach it once `listen` hooks the keyboard.
    pub fn new(parts: DictationParts) -> Self {
        let (events, inbox) = mpsc::channel();
        let polishing = Arc::new(AtomicBool::new(false));
        let hotkey_vk = parts.config.hotkey_vk();
        let (transcriptions, queue) = mpsc::channel();
        let transcriber = parts.transcriber.clone();
        let results = events.clone();
        let worker = thread::Builder::new()
            .name("transcribe".to_owned())
            .spawn(move || transcribe_each(&queue, transcriber.as_ref(), &results))
            .expect("a thread starts");
        let controller = Controller {
            parts,
            events: events.clone(),
            state: IDLE,
            released_at: 0.0,
            instructions: String::new(),
            polishing: polishing.clone(),
            transcriptions: Some(transcriptions),
            worker: Some(worker),
        };
        let ended = Arc::new(Ended::default());
        let end = ended.clone();
        let controller = thread::Builder::new()
            .name("controller".to_owned())
            .spawn(move || {
                controller.run(&inbox);
                end.set();
            })
            .expect("a thread starts");
        Self { hotkey_vk, events, polishing, controller: Mutex::new(Some(controller)), ended, hook: Mutex::new(None) }
    }

    pub fn key_filter(&self) -> KeyFilter {
        KeyFilter {
            hotkey_vk: self.hotkey_vk,
            events: self.events.clone(),
            polishing: self.polishing.clone(),
            hotkey_held: false,
            esc_swallowed: false,
        }
    }

    pub fn listen(&self) -> Result<(), String> {
        let mut filter = self.key_filter();
        let hook = KeyboardHook::start(move |key| filter.on_key(key))?;
        *self.hook.lock().unwrap() = Some(hook);
        Ok(())
    }

    pub fn post(&self, event: Event) {
        let _ = self.events.send(Message::Event(event));
    }

    /// Wait for the current dictation and its hotkey release, then end the controller.
    pub fn finish(&self) {
        let _ = self.events.send(Message::Finish);
        self.ended.wait(Duration::MAX);
    }

    pub fn stop(&self) {
        if let Some(mut hook) = self.hook.lock().unwrap().take() {
            hook.stop();
        }
        let _ = self.events.send(Message::Stop);
        // A paste or a transcription that never returns must not keep the app from quitting.
        if self.ended.wait(STOP_TIMEOUT)
            && let Some(controller) = self.controller.lock().unwrap().take()
        {
            let _ = controller.join();
        }
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        self.stop();
    }
}

fn transcribe_each(queue: &Receiver<Audio>, transcriber: &dyn Transcribe, results: &Sender<Message>) {
    let mut request_ids = 1..;
    for audio in queue {
        let started = clock::now();
        let event = match transcriber.transcribe(&audio) {
            Ok(text) => {
                let ms = (clock::now() - started) * 1000.0;
                info!("transcribed {:.1} s at {} Hz in {ms:.0} ms: {} chars", audio.seconds(), audio.rate, text.chars().count());
                Event::TranscriptReady { text, request_id: request_ids.next().expect("an endless range") }
            }
            Err(e) => {
                error!("transcription failed: {e}");
                Event::RecordingFailed(e)
            }
        };
        let _ = results.send(Message::Event(event));
    }
}

struct Controller {
    parts: DictationParts,
    events: Sender<Message>,
    state: State,
    released_at: f64,
    instructions: String,
    polishing: Arc<AtomicBool>,
    transcriptions: Option<Sender<Audio>>,
    worker: Option<JoinHandle<()>>,
}

impl Controller {
    fn run(mut self, inbox: &Receiver<Message>) {
        let mut finishing = false;
        loop {
            if finishing && self.state == IDLE {
                break;
            }
            let message = if matches!(self.state, State::AwaitingRelease { .. }) {
                match inbox.recv_timeout(RELEASE_POLL) {
                    Ok(message) => message,
                    Err(RecvTimeoutError::Timeout) => {
                        if (self.parts.hotkey_down)() {
                            continue;
                        }
                        warn!("hotkey release was not seen by the hook; using the key state");
                        Message::Event(Event::HotkeyUp(clock::now()))
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match inbox.recv() {
                    Ok(message) => message,
                    Err(_) => break,
                }
            };
            match message {
                Message::Stop => break,
                Message::Finish => finishing = true,
                Message::Event(event) => self.handle(event),
            }
        }
        self.parts.recorder.discard();
        drop(self.transcriptions.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn polish_on(&self) -> bool {
        self.parts.polisher.is_some() && ((self.parts.polish_enabled)() || !self.instructions.is_empty())
    }

    fn read_rules(&self) -> String {
        if self.parts.config.app_rules.is_empty() {
            return String::new();
        }
        combined_instructions(&self.parts.config.app_rules, (self.parts.foreground_app)().as_deref())
    }

    fn handle(&mut self, event: Event) {
        if let Event::HotkeyUp(t) = event {
            self.released_at = t;
        }
        let tap_threshold = f64::from(self.parts.config.tap_threshold_ms) / 1000.0;
        let (state, effects) = step(&self.state, &event, tap_threshold, self.polish_on());
        let previous = std::mem::replace(&mut self.state, state);
        self.polishing.store(matches!(self.state, State::Polishing { .. }), Ordering::Release);
        if self.state != previous {
            info!("{:?} -> {:?} on {:?}", previous, self.state, event);
            if let Event::PolishFailed { reason, .. } = &event {
                warn!("post-processing failed ({reason}); pasting the raw transcript");
            }
            if let (Event::OtherKeyDown, State::Recording { pressed_at, .. }) = (&event, &previous) {
                // Data for the overlay's show delay: how soon a real shortcut's second key arrives.
                info!("shortcut: another key {:.0} ms after the hotkey", (clock::now() - pressed_at) * 1000.0);
            }
            // Before the effects run: a paste blocks this thread for the clipboard restore, and the
            // observers should see the transition when it happens.
            let transition = Transition { previous, state: self.state.clone(), event, effects: effects.clone() };
            (self.parts.on_transition)(&transition);
        }
        for effect in effects {
            self.execute(effect);
        }
    }

    fn post(&self, event: Event) {
        let _ = self.events.send(Message::Event(event));
    }

    fn execute(&mut self, effect: Effect) {
        match effect {
            Effect::StartRecording => {
                self.instructions = self.read_rules();
                match self.parts.recorder.start() {
                    Err(e) => {
                        error!("microphone did not start: {e}");
                        self.post(Event::RecordingFailed(e));
                    }
                    Ok(()) => {
                        info!("recording from '{}'", self.parts.recorder.device_name());
                        if self.polish_on()
                            && let Some(polisher) = self.parts.polisher.clone()
                        {
                            thread::spawn(move || polisher.preconnect());
                        }
                    }
                }
            }
            Effect::StopAndTranscribe => {
                self.instructions = self.read_rules();
                if !self.instructions.is_empty() && self.parts.polisher.is_none() {
                    warn!("app rules need {}; pasting the raw transcript", self.parts.config.postprocess.api_key_name());
                }
                match self.parts.recorder.stop() {
                    Err(e) => {
                        error!("microphone did not stop cleanly: {e}");
                        self.post(Event::RecordingFailed(e));
                    }
                    Ok(audio) => {
                        if let Some(transcriptions) = &self.transcriptions {
                            let _ = transcriptions.send(audio);
                        }
                    }
                }
            }
            Effect::DiscardRecording => {
                self.instructions.clear();
                self.parts.recorder.discard();
            }
            Effect::Paste(text) => {
                let text = self.parts.config.dictionary.apply(&text);
                let since_release_ms = (clock::now() - self.released_at) * 1000.0;
                let spaced = if self.parts.config.append_space { format!("{text} ") } else { text.clone() };
                match (self.parts.paste)(&spaced) {
                    Err(e) => {
                        error!("paste failed: {e}");
                        self.parts.cues.play(Cue::Error);
                    }
                    Ok(()) => info!("pasted {} chars, sent {since_release_ms:.0} ms after the hotkey release", text.chars().count()),
                }
            }
            Effect::PlaySound(cue) => self.parts.cues.play(cue),
            Effect::Polish { text, request_id } => {
                let Some(polisher) = self.parts.polisher.clone() else {
                    return;
                };
                // Capture now: Esc can start another dictation before a queued request runs.
                let instructions = self.parts.config.dictionary.instructions(&self.instructions);
                let results = self.events.clone();
                // Its own thread, so a request still running after Esc never delays the next dictation's.
                thread::spawn(move || {
                    let event = match polisher.polish(&text, &instructions) {
                        Ok(text) => Event::Polished { request_id, text },
                        Err(e) => Event::PolishFailed { request_id, reason: e.to_string() },
                    };
                    let _ = results.send(Message::Event(event));
                });
                // This timer bounds the whole wait, DNS lookup included. When the request finishes first, the
                // reducer ignores the timer's PolishFailed because its id is no longer current.
                let timeout_s = self.parts.config.postprocess.timeout_s;
                let results = self.events.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs_f64(timeout_s));
                    let reason = format!("no reply within {} s", short_float(timeout_s));
                    let _ = results.send(Message::Event(Event::PolishFailed { request_id, reason }));
                });
            }
        }
    }
}
