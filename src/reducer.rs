//! The dictation state machine: pure, clock-free, and the only place that decides what a key press means.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Hold,
    Toggle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cue {
    Start,
    Stop,
    Cancel,
    Error,
}

#[derive(Clone, PartialEq)]
pub enum State {
    Idle { key_down: bool },
    Recording { mode: Mode, pressed_at: f64 },
    Transcribing { key_down: bool },
    Polishing { raw: String, request_id: u64, key_down: bool },
    AwaitingRelease { text: String },
}

pub const IDLE: State = State::Idle { key_down: false };

impl State {
    pub fn is_idle(&self) -> bool {
        matches!(self, State::Idle { .. })
    }
}

// Hand-written so transcript text can never reach the log through a state or an event.
impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            State::Idle { key_down } => write!(f, "Idle(key_down={key_down})"),
            State::Recording { mode, pressed_at } => write!(f, "Recording(mode={mode:?}, pressed_at={pressed_at})"),
            State::Transcribing { key_down } => write!(f, "Transcribing(key_down={key_down})"),
            State::Polishing { request_id, key_down, .. } => write!(f, "Polishing(request_id={request_id}, key_down={key_down})"),
            State::AwaitingRelease { .. } => write!(f, "AwaitingRelease()"),
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum Event {
    HotkeyDown(f64),
    HotkeyUp(f64),
    OtherKeyDown,
    Cancel,
    TranscriptReady { text: String, request_id: u64 },
    RecordingFailed(String),
    Polished { request_id: u64, text: String },
    PolishFailed { request_id: u64, reason: String },
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::HotkeyDown(t) => write!(f, "HotkeyDown(t={t})"),
            Event::HotkeyUp(t) => write!(f, "HotkeyUp(t={t})"),
            Event::OtherKeyDown => write!(f, "OtherKeyDown()"),
            Event::Cancel => write!(f, "Cancel()"),
            Event::TranscriptReady { request_id, .. } => write!(f, "TranscriptReady(request_id={request_id})"),
            Event::RecordingFailed(error) => write!(f, "RecordingFailed(error={error:?})"),
            Event::Polished { request_id, .. } => write!(f, "Polished(request_id={request_id})"),
            Event::PolishFailed { request_id, reason } => write!(f, "PolishFailed(request_id={request_id}, reason={reason:?})"),
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum Effect {
    StartRecording,
    StopAndTranscribe,
    DiscardRecording,
    Paste(String),
    PlaySound(Cue),
    Polish { text: String, request_id: u64 },
}

impl fmt::Debug for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::StartRecording => write!(f, "StartRecording()"),
            Effect::StopAndTranscribe => write!(f, "StopAndTranscribe()"),
            Effect::DiscardRecording => write!(f, "DiscardRecording()"),
            Effect::Paste(_) => write!(f, "Paste()"),
            Effect::PlaySound(cue) => write!(f, "PlaySound({cue:?})"),
            Effect::Polish { request_id, .. } => write!(f, "Polish(request_id={request_id})"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub previous: State,
    pub state: State,
    pub event: Event,
    pub effects: Vec<Effect>,
}

pub const DEFAULT_TAP_THRESHOLD_S: f64 = 0.3;

pub fn step(state: &State, event: &Event, tap_threshold: f64, polish: bool) -> (State, Vec<Effect>) {
    use Effect::{DiscardRecording, Paste, PlaySound, StartRecording, StopAndTranscribe};
    match (state, event) {
        (State::Idle { key_down: false }, Event::HotkeyDown(t)) => {
            (State::Recording { mode: Mode::Hold, pressed_at: *t }, vec![StartRecording, PlaySound(Cue::Start)])
        }
        (State::Idle { .. }, Event::HotkeyUp(_)) => (IDLE, vec![]),

        (State::Recording { mode: Mode::Hold, pressed_at }, Event::HotkeyUp(t)) => {
            if t - pressed_at < tap_threshold {
                (State::Recording { mode: Mode::Toggle, pressed_at: *pressed_at }, vec![])
            } else {
                (State::Transcribing { key_down: false }, vec![StopAndTranscribe, PlaySound(Cue::Stop)])
            }
        }
        (State::Recording { mode: Mode::Toggle, .. }, Event::HotkeyDown(_)) => {
            (State::Transcribing { key_down: true }, vec![StopAndTranscribe, PlaySound(Cue::Stop)])
        }
        (State::Recording { mode: Mode::Hold, .. }, Event::OtherKeyDown) => (State::Idle { key_down: true }, vec![DiscardRecording]),
        (State::Recording { mode, .. }, Event::Cancel) => {
            (State::Idle { key_down: *mode == Mode::Hold }, vec![DiscardRecording, PlaySound(Cue::Cancel)])
        }
        (State::Recording { mode, .. }, Event::RecordingFailed(_)) => {
            (State::Idle { key_down: *mode == Mode::Hold }, vec![DiscardRecording, PlaySound(Cue::Error)])
        }

        (State::Transcribing { .. }, Event::HotkeyDown(_)) => (State::Transcribing { key_down: true }, vec![]),
        (State::Transcribing { .. }, Event::HotkeyUp(_)) => (State::Transcribing { key_down: false }, vec![]),
        (State::Polishing { raw, request_id, .. }, Event::HotkeyDown(_)) => {
            (State::Polishing { raw: raw.clone(), request_id: *request_id, key_down: true }, vec![])
        }
        (State::Polishing { raw, request_id, .. }, Event::HotkeyUp(_)) => {
            (State::Polishing { raw: raw.clone(), request_id: *request_id, key_down: false }, vec![])
        }
        (State::Transcribing { key_down }, Event::TranscriptReady { text, request_id }) => {
            if text.is_empty() {
                (State::Idle { key_down: *key_down }, vec![])
            } else if polish {
                (
                    State::Polishing { raw: text.clone(), request_id: *request_id, key_down: *key_down },
                    vec![Effect::Polish { text: text.clone(), request_id: *request_id }],
                )
            } else {
                deliver(text, *key_down)
            }
        }
        (State::Transcribing { key_down }, Event::RecordingFailed(_)) => (State::Idle { key_down: *key_down }, vec![PlaySound(Cue::Error)]),

        // A result whose id is not the current request belongs to a dictation that was already pasted; it
        // falls through and is ignored.
        (State::Polishing { request_id: current, key_down, .. }, Event::Polished { request_id, text }) if request_id == current => {
            deliver(text, *key_down)
        }
        (State::Polishing { raw, request_id: current, key_down }, Event::PolishFailed { request_id, .. }) if request_id == current => {
            deliver(raw, *key_down)
        }
        (State::Polishing { raw, key_down, .. }, Event::Cancel) => deliver(raw, *key_down),

        (State::AwaitingRelease { text }, Event::HotkeyUp(_)) => (IDLE, vec![Paste(text.clone())]),
        (State::AwaitingRelease { .. }, Event::Cancel) => (State::Idle { key_down: true }, vec![PlaySound(Cue::Cancel)]),

        _ => (state.clone(), vec![]),
    }
}

fn deliver(text: &str, key_down: bool) -> (State, Vec<Effect>) {
    if key_down {
        (State::AwaitingRelease { text: text.to_owned() }, vec![])
    } else {
        (IDLE, vec![Effect::Paste(text.to_owned())])
    }
}
