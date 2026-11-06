use std::collections::HashMap;
use std::f64::consts::TAU;

use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

use crate::reducer::Cue;

const RATE: u32 = 44_100;
const NOTE_S: f64 = 0.06;
const GAP_S: f64 = 0.02;
const VOLUME: f64 = 0.12;

pub trait PlayCue: Send + Sync {
    fn play(&self, cue: Cue);
}

fn notes_hz(cue: Cue) -> &'static [u32] {
    match cue {
        Cue::Start => &[600, 900],
        Cue::Stop => &[900, 600],
        Cue::Cancel => &[450],
        Cue::Error => &[300, 300],
    }
}

/// A mono 16-bit WAV file of the notes, each with a short fade and a gap.
pub fn tone(notes: &[u32]) -> Vec<u8> {
    let n = (NOTE_S * f64::from(RATE)) as usize;
    let fade = (0.008 * f64::from(RATE)) as usize as f64;
    let gap = (GAP_S * f64::from(RATE)) as usize;
    let mut samples: Vec<i16> = Vec::with_capacity(notes.len() * (n + gap));
    for hz in notes {
        for i in 0..n {
            let envelope = (i as f64 / fade).min((n - i) as f64 / fade).min(1.0);
            let wave = (TAU * f64::from(*hz) * i as f64 / f64::from(RATE)).sin();
            samples.push((32767.0 * VOLUME * envelope * wave) as i16);
        }
        samples.extend(std::iter::repeat_n(0, gap));
    }
    let bytes = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

pub struct Cues {
    // Leaked on purpose: Windows reads a sound from memory after PlaySound returns, for as long as it plays.
    sounds: HashMap<Cue, &'static [u8]>,
}

impl Cues {
    pub fn new(enabled: bool) -> Self {
        let mut sounds = HashMap::new();
        if enabled {
            for cue in [Cue::Start, Cue::Stop, Cue::Cancel, Cue::Error] {
                sounds.insert(cue, &*Vec::leak(tone(notes_hz(cue))));
            }
        }
        Self { sounds }
    }
}

impl PlayCue for Cues {
    fn play(&self, cue: Cue) {
        if let Some(sound) = self.sounds.get(&cue) {
            unsafe { PlaySoundW(sound.as_ptr().cast(), std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
        }
    }
}
