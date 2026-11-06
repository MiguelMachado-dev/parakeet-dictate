//! Port of tests/test_styles.py.

use parakeet_dictate::config::{DEFAULT_STYLE, STYLES};
use parakeet_dictate::meter::{LEVEL_HISTORY, SPECTRUM_BANDS};
use parakeet_dictate::overlay::Style;
use parakeet_dictate::overlay::frame::{BEAT_S, OverlayFrame, Phase};

const PHASES: [Phase; 6] = [Phase::Recording, Phase::Transcribing, Phase::Polishing, Phase::Done, Phase::Cancelled, Phase::Failed];

/// A small generator with a fixed seed, so a run is repeatable.
struct Random(u64);

impl Random {
    fn uniform(&mut self, low: f32, high: f32) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        low + (high - low) * (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

struct Audio {
    levels: Vec<f32>,
    spectrum: Vec<f32>,
}

fn loud() -> Audio {
    let mut random = Random(7);
    Audio {
        levels: (0..LEVEL_HISTORY).map(|_| random.uniform(0.3, 1.0)).collect(),
        spectrum: (0..SPECTRUM_BANDS).map(|_| random.uniform(0.2, 1.0)).collect(),
    }
}

fn silent() -> Audio {
    Audio { levels: vec![0.0; LEVEL_HISTORY], spectrum: vec![0.0; SPECTRUM_BANDS] }
}

fn frame(phase: Phase, presence: f64, scale: f64, hands_free: bool, audio: &Audio) -> OverlayFrame<'_> {
    let beat = matches!(phase, Phase::Done | Phase::Cancelled | Phase::Failed);
    OverlayFrame {
        phase,
        phase_elapsed: if beat { BEAT_S / 2.0 } else { 0.7 },
        presence,
        hands_free,
        recording_elapsed: 2.0,
        levels: &audio.levels,
        spectrum: &audio.spectrum,
        scale,
    }
}

fn styles() -> impl Iterator<Item = Style> {
    STYLES.iter().map(|name| Style::named(name).expect("a registered style"))
}

fn render(style: Style, frame: &OverlayFrame<'_>) -> Vec<u8> {
    let (width, height) = frame.size();
    let mut image = vec![0xaa; width * height * 4];
    style.render(frame, &mut image);
    image
}

#[test]
fn test_the_default_style_is_registered() {
    assert!(STYLES.contains(&DEFAULT_STYLE));
}

#[test]
fn test_every_phase_draws_inside_the_canvas() {
    let audio = loud();
    for style in styles() {
        for phase in PHASES {
            for scale in [1.0, 1.5] {
                let f = frame(phase, 1.0, scale, phase == Phase::Recording, &audio);
                let image = render(style, &f);
                let (width, height) = f.size();
                let alpha = |x: usize, y: usize| image[(y * width + x) * 4 + 3];
                let body = image.chunks_exact(4).map(|pixel| pixel[3]).max().unwrap();
                assert!(body > 200, "the body is visible: {style:?} {phase:?} {scale}");
                let top_and_bottom = (0..width).flat_map(|x| [alpha(x, 0), alpha(x, height - 1)]);
                let sides = (0..height).flat_map(|y| [alpha(0, y), alpha(width - 1, y)]);
                let border = top_and_bottom.chain(sides).max().unwrap();
                assert_eq!(border, 0, "nothing reaches the canvas edge: {style:?} {phase:?} {scale}");
            }
        }
    }
}

#[test]
fn test_presence_zero_is_fully_transparent() {
    let audio = loud();
    for style in styles() {
        let image = render(style, &frame(Phase::Recording, 0.0, 1.0, false, &audio));
        assert!(image.chunks_exact(4).all(|pixel| pixel[3] == 0), "{style:?}");
    }
}

#[test]
fn test_recording_reacts_to_the_voice() {
    let (voice, quiet) = (loud(), silent());
    for style in styles() {
        let loud_image = render(style, &frame(Phase::Recording, 1.0, 1.0, false, &voice));
        let silent_image = render(style, &frame(Phase::Recording, 1.0, 1.0, false, &quiet));
        let total: u64 = loud_image.iter().zip(&silent_image).map(|(a, b)| u64::from(a.abs_diff(*b))).sum();
        let mean = total as f64 / loud_image.len() as f64;
        assert!(mean > 1.0, "{style:?} differs by only {mean}");
    }
}
