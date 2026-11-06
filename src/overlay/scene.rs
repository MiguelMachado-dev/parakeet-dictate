//! What the overlay shows and when. Maps dictation transitions to commands and plays them out
//! over time. Pure and clock-free, so tests drive it without a window.

use super::frame::{BEAT_S, CANVAS_SIZE, OverlayFrame, Phase, scaled};
use crate::reducer::{Cue, Effect, Event, Mode, State, Transition};

// A recording only appears after this long, so the hotkey pressed as part of a shortcut such as
// Right Ctrl+C never flashes the window. The start tone is the instant feedback.
pub const SHOW_DELAY_S: f64 = 0.25;
pub const FADE_IN_S: f64 = 0.15;
pub const FADE_OUT_S: f64 = 0.2;
// Logical px between the bottom of the canvas and the bottom of the work area.
const BOTTOM_MARGIN: f64 = 48.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Show { phase: Phase, hands_free: bool },
    /// Ends the dictation: plays the beat for BEAT_S, then fades out. No beat fades out at once.
    Finish { beat: Option<Phase> },
    /// Ends the dictation with no beat and no fade.
    Vanish,
}

pub fn command_for(transition: &Transition) -> Option<Command> {
    match transition.state {
        State::Recording { mode, .. } => Some(Command::Show { phase: Phase::Recording, hands_free: mode == Mode::Toggle }),
        State::Transcribing { .. } | State::AwaitingRelease { .. } => Some(Command::Show { phase: Phase::Transcribing, hands_free: false }),
        State::Polishing { .. } => Some(Command::Show { phase: Phase::Polishing, hands_free: false }),
        State::Idle { .. } if transition.previous.is_idle() => None,
        State::Idle { .. } if transition.event == Event::OtherKeyDown => Some(Command::Vanish),
        State::Idle { .. } => Some(Command::Finish { beat: beat(&transition.effects) }),
    }
}

fn beat(effects: &[Effect]) -> Option<Phase> {
    effects.iter().find_map(|effect| match effect {
        Effect::Paste(_) => Some(Phase::Done),
        Effect::PlaySound(Cue::Cancel) => Some(Phase::Cancelled),
        Effect::PlaySound(Cue::Error) => Some(Phase::Failed),
        _ => None,
    })
}

/// Physical pixels on the virtual screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
}

/// Bottom center of a monitor's work area (left, top, right, bottom) at the monitor's DPI.
pub fn place(work: (i32, i32, i32, i32), dpi: u32) -> Placement {
    let scale = f64::from(dpi) / 96.0;
    let (width, height) = (scaled(CANVAS_SIZE.0, scale) as i32, scaled(CANVAS_SIZE.1, scale) as i32);
    let (left, _, right, bottom) = work;
    let margin = (BOTTOM_MARGIN * scale).round_ties_even() as i32;
    Placement { x: left + (right - left - width).div_euclid(2), y: bottom - margin - height, width, height, scale }
}

/// One dictation on screen, from its fade-in to the end of its fade-out.
#[derive(Clone, Copy, Debug)]
struct Appearance {
    shows_at: f64,
    recording_started: f64,
    placement: Placement,
    phase: Phase,
    phase_started: f64,
    hands_free: bool,
    recording_ended: Option<f64>,
    fades_at: Option<f64>,
    /// Presence at shows_at: above 0 when the previous dictation was still fading out.
    rise_from: f64,
}

impl Appearance {
    fn rising(&self, t: f64) -> f64 {
        (self.rise_from + (t - self.shows_at) / FADE_IN_S).min(1.0)
    }

    fn presence(&self, now: f64) -> f64 {
        if now < self.shows_at {
            return 0.0;
        }
        match self.fades_at {
            Some(fades_at) if now >= fades_at => (self.rising(fades_at) - (now - fades_at) / FADE_OUT_S).max(0.0),
            _ => self.rising(now),
        }
    }

    fn gone(&self, now: f64) -> bool {
        self.fades_at.is_some_and(|fades_at| now >= fades_at) && self.presence(now) <= 0.0
    }
}

#[derive(Debug, Default)]
pub struct Scene {
    /// The latest dictation, possibly still waiting for SHOW_DELAY_S.
    current: Option<Appearance>,
    /// The dictation before it, still ending on screen while `current` waits.
    outgoing: Option<Appearance>,
}

impl Scene {
    /// `placement` is where a dictation that starts with this command will show.
    pub fn apply(&mut self, command: Command, t: f64, placement: Placement) {
        self.advance(t);
        let Some(current) = self.current.as_mut().filter(|current| current.fades_at.is_none()) else {
            if let Command::Show { phase, hands_free } = command {
                self.outgoing = self.current;
                let shows_at = t + SHOW_DELAY_S;
                self.current = Some(Appearance {
                    shows_at,
                    recording_started: t,
                    placement,
                    phase,
                    phase_started: shows_at,
                    hands_free,
                    recording_ended: None,
                    fades_at: None,
                    rise_from: 0.0,
                });
            }
            return;
        };
        match command {
            Command::Show { phase, hands_free } => {
                if phase != current.phase {
                    current.phase = phase;
                    current.phase_started = t.max(current.shows_at);
                }
                if phase != Phase::Recording && current.recording_ended.is_none() {
                    current.recording_ended = Some(t);
                }
                current.hands_free |= hands_free;
            }
            // Never shown, so it never will be: whatever was ending on screen plays on untouched.
            Command::Finish { .. } | Command::Vanish if t < current.shows_at => {
                self.current = self.outgoing.take();
            }
            Command::Finish { beat } => {
                current.recording_ended.get_or_insert(t);
                match beat {
                    None => current.fades_at = Some(t),
                    Some(beat) => {
                        current.phase = beat;
                        current.phase_started = t;
                        current.fades_at = Some(t + BEAT_S);
                    }
                }
            }
            Command::Vanish => self.current = None,
        }
    }

    /// The frame to draw at `now`, or None when the window should be hidden.
    pub fn frame<'a>(&mut self, now: f64, levels: &'a [f32], spectrum: &'a [f32]) -> Option<(OverlayFrame<'a>, Placement)> {
        self.advance(now);
        let shown = self.outgoing.as_ref().or(self.current.as_ref()).filter(|shown| now >= shown.shows_at)?;
        let frame = OverlayFrame {
            phase: shown.phase,
            phase_elapsed: now - shown.phase_started,
            presence: shown.presence(now),
            hands_free: shown.hands_free,
            recording_elapsed: shown.recording_ended.unwrap_or(now) - shown.recording_started,
            levels,
            spectrum,
            scale: shown.placement.scale,
        };
        Some((frame, shown.placement))
    }

    /// When `frame` returns None: the time a dictation waiting for SHOW_DELAY_S appears, if any.
    pub fn next_show(&self) -> Option<f64> {
        self.current.as_ref().map(|current| current.shows_at)
    }

    fn advance(&mut self, now: f64) {
        if let (Some(current), Some(outgoing)) = (self.current.as_mut(), self.outgoing.as_ref())
            && now >= current.shows_at
        {
            current.rise_from = outgoing.presence(current.shows_at);
            self.outgoing = None;
        }
        if self.outgoing.is_some_and(|outgoing| outgoing.gone(now)) {
            self.outgoing = None;
        }
        if self.current.is_some_and(|current| current.gone(now)) {
            self.current = None;
        }
    }
}
