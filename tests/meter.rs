//! Port of tests/test_meter.py.

use std::f64::consts::{PI, SQRT_2};

use parakeet_dictate::meter::{CURVE, HOP_S, LEVEL_HISTORY, LevelMeter, SPECTRUM_BANDS};

const RATE: u32 = 44_100;

fn tone_at(db: f64, seconds: f64, hz: f64) -> Vec<f32> {
    let rate = f64::from(RATE);
    let amplitude = SQRT_2 * 10f64.powf(db / 20.0);
    (0..(rate * seconds).round_ties_even() as usize)
        .map(|n| (amplitude * (2.0 * PI * hz * (n as f64 / rate)).sin()) as f32)
        .collect()
}

fn tone(db: f64, seconds: f64) -> Vec<f32> {
    tone_at(db, seconds, 440.0)
}

fn silence() -> Vec<f32> {
    vec![0.0; RATE as usize]
}

fn max(values: &[f32]) -> f32 {
    values.iter().copied().fold(f32::NEG_INFINITY, f32::max)
}

/// The parts of `np.array_split(signal, sections)`: the first ones take the remainder.
fn array_split(signal: &[f32], sections: usize) -> Vec<&[f32]> {
    let (size, extra) = (signal.len() / sections, signal.len() % sections);
    let mut rest = signal;
    (0..sections)
        .map(|section| {
            let (chunk, tail) = rest.split_at(size + usize::from(section < extra));
            rest = tail;
            chunk
        })
        .collect()
}

#[test]
fn test_silence_reads_zero() {
    let meter = LevelMeter::new(RATE);
    meter.feed(&silence());
    let (levels, spectrum) = meter.snapshot();
    assert_eq!((levels.len(), spectrum.len()), (LEVEL_HISTORY, SPECTRUM_BANDS));
    assert_eq!((max(&levels), max(&spectrum)), (0.0, 0.0));
}

#[test]
fn test_room_noise_below_the_floor_reads_zero() {
    let meter = LevelMeter::new(RATE);
    meter.feed(&tone(-87.0, 1.0));
    assert_eq!(max(&meter.snapshot().0), 0.0);
}

#[test]
fn test_a_quiet_mic_still_fills_most_of_the_scale() {
    let meter = LevelMeter::new(RATE);
    meter.feed(&tone(-40.0, 1.0));
    let last = f64::from(meter.snapshot().0[LEVEL_HISTORY - 1]);
    let want = (20.0f64 / 25.0).powf(CURVE);
    assert!((last - want).abs() <= 0.02, "{last} is not {want}");
}

#[test]
fn test_a_loud_mic_moves_the_ceiling_up_to_its_peak() {
    let meter = LevelMeter::new(RATE);
    meter.feed(&tone(-12.0, 1.0));
    let last = f64::from(meter.snapshot().0[LEVEL_HISTORY - 1]);
    assert!((last - 1.0).abs() <= 0.02, "{last} is not 1");
}

#[test]
fn test_history_is_oldest_first() {
    let meter = LevelMeter::new(RATE);
    meter.feed(&silence());
    meter.feed(&tone(-20.0, 10.0 * HOP_S));
    let levels = meter.snapshot().0;
    assert_eq!(levels[0], 0.0);
    assert!(levels[LEVEL_HISTORY - 1] > 0.5);
}

#[test]
fn test_the_tone_lands_in_its_band() {
    let meter = LevelMeter::new(RATE);
    meter.feed(&tone_at(-20.0, 0.5, 1000.0));
    let spectrum = meter.snapshot().1;
    let loudest = spectrum.iter().position(|value| *value == max(&spectrum)).unwrap();
    let ratio = (8000.0f64 / 80.0).powf(1.0 / SPECTRUM_BANDS as f64);
    let edges: Vec<f64> = (0..=SPECTRUM_BANDS).map(|i| 80.0 * ratio.powi(i as i32)).collect();
    assert_eq!(loudest, edges.partition_point(|edge| *edge <= 1000.0) - 1);
}

#[test]
fn test_feeding_in_odd_block_sizes_matches_one_block() {
    let signal = tone(-25.0, 0.7);
    let (whole, pieces) = (LevelMeter::new(RATE), LevelMeter::new(RATE));
    whole.feed(&signal);
    for chunk in array_split(&signal, 37) {
        pieces.feed(chunk);
    }
    let (want, got) = (whole.snapshot().0, pieces.snapshot().0);
    for (index, (want, got)) in want.iter().zip(&got).enumerate() {
        // The tolerance of np.testing.assert_allclose(atol=1e-6) with its default rtol.
        assert!((got - want).abs() <= 1e-6 + 1e-7 * want.abs(), "level {index}: {got} is not {want}");
    }
}

#[test]
fn test_reset_clears_history() {
    let meter = LevelMeter::new(RATE);
    meter.feed(&tone(-20.0, 0.5));
    meter.reset();
    assert_eq!(max(&meter.snapshot().0), 0.0);
}
