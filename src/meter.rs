//! Live microphone level and spectrum for the overlay, fed from the audio callback.

use std::sync::{Arc, Mutex};

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

pub const HOP_S: f64 = 0.02;
pub const LEVEL_HISTORY: usize = 96;
pub const SPECTRUM_BANDS: usize = 24;
const FFT_SIZE: usize = 2048;
const BAND_RANGE_HZ: (f64, f64) = (80.0, 8000.0);
const FLOOR_DB: f64 = -60.0;
// The ceiling follows the loudest recent hop, but never drops below this, so a quiet mic still
// fills the scale and room noise does not.
const MIN_CEILING_DB: f64 = -35.0;
const PEAK_DECAY_DB_PER_S: f64 = 3.0;
// Speech sits in the top quarter of the dB range (fixture median -21 dBFS, peak -13), so a
// linear map pins the bars near full. The curve spreads that quarter over most of the scale.
pub const CURVE: f64 = 2.5;
const ATTACK: f64 = 0.6;
const RELEASE: f64 = 0.15;
const EPS: f64 = 1e-12;

pub type Levels = [f32; LEVEL_HISTORY];
pub type Spectrum = [f32; SPECTRUM_BANDS];

fn db(power: f64) -> f64 {
    10.0 * (power + EPS).log10()
}

fn curved(value_db: f64, ceiling: f64) -> f64 {
    ((value_db - FLOOR_DB) / (ceiling - FLOOR_DB)).clamp(0.0, 1.0).powf(CURVE)
}

struct Live {
    levels: Levels,
    spectrum: Spectrum,
    tail: Box<[f32; FFT_SIZE]>,
    pending: usize,
    hop_energy: f64,
    level: f64,
    peak_db: f64,
    buffer: Vec<Complex32>,
    scratch: Vec<Complex32>,
}

impl Live {
    fn new(fft: &dyn Fft<f32>) -> Self {
        Self {
            levels: [0.0; LEVEL_HISTORY],
            spectrum: [0.0; SPECTRUM_BANDS],
            tail: Box::new([0.0; FFT_SIZE]),
            pending: 0,
            hop_energy: 0.0,
            level: 0.0,
            peak_db: MIN_CEILING_DB,
            buffer: vec![Complex32::default(); FFT_SIZE],
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
        }
    }
}

/// Single writer (the audio callback) and single reader (the overlay thread).
pub struct LevelMeter {
    pub rate: u32,
    hop: usize,
    window: Vec<f32>,
    scale: f32,
    /// Band of each FFT bin up to Nyquist, None outside the band range.
    band_of_bin: Vec<Option<usize>>,
    fft: Arc<dyn Fft<f32>>,
    live: Mutex<Live>,
}

impl LevelMeter {
    pub fn new(rate: u32) -> Self {
        let hop = ((f64::from(rate) * HOP_S).round_ties_even() as usize).max(1);
        let last = (FFT_SIZE - 1) as f64;
        let window: Vec<f32> =
            (0..FFT_SIZE).map(|n| (0.5 - 0.5 * (std::f64::consts::TAU * n as f64 / last).cos()) as f32).collect();
        // Scaled so the band powers sum to the frame's mean power, which keeps bands on the
        // same dB scale as the level.
        let energy: f32 = window.iter().map(|value| value * value).sum();
        let scale = 2.0 / (FFT_SIZE as f32 * energy);
        let (low, high) = (BAND_RANGE_HZ.0.log10(), BAND_RANGE_HZ.1.log10());
        let mut edges: Vec<f64> =
            (0..=SPECTRUM_BANDS).map(|i| 10f64.powf(low + (high - low) * i as f64 / SPECTRUM_BANDS as f64)).collect();
        edges[0] = BAND_RANGE_HZ.0;
        edges[SPECTRUM_BANDS] = BAND_RANGE_HZ.1;
        let band_of_bin = (0..=FFT_SIZE / 2)
            .map(|bin| {
                let hz = bin as f64 * f64::from(rate) / FFT_SIZE as f64;
                edges.partition_point(|edge| *edge <= hz).checked_sub(1).filter(|band| *band < SPECTRUM_BANDS)
            })
            .collect();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let live = Mutex::new(Live::new(fft.as_ref()));
        Self { rate, hop, window, scale, band_of_bin, fft, live }
    }

    pub fn reset(&self) {
        *self.live.lock().unwrap() = Live::new(self.fft.as_ref());
    }

    pub fn feed(&self, mut block: &[f32]) {
        let mut live = self.live.lock().unwrap();
        while !block.is_empty() {
            let take = block.len().min(self.hop - live.pending);
            let (now, rest) = block.split_at(take);
            block = rest;
            live.hop_energy += now.iter().map(|sample| f64::from(*sample) * f64::from(*sample)).sum::<f64>();
            let kept = take.min(FFT_SIZE);
            live.tail.copy_within(kept.., 0);
            live.tail[FFT_SIZE - kept..].copy_from_slice(&now[take - kept..]);
            live.pending += take;
            if live.pending == self.hop {
                live.pending = 0;
                self.analyze_hop(&mut live);
            }
        }
    }

    pub fn snapshot(&self) -> (Levels, Spectrum) {
        let live = self.live.lock().unwrap();
        (live.levels, live.spectrum)
    }

    fn analyze_hop(&self, live: &mut Live) {
        let rms_db = db(live.hop_energy / self.hop as f64);
        live.hop_energy = 0.0;
        live.peak_db = (live.peak_db - PEAK_DECAY_DB_PER_S * HOP_S).max(rms_db);
        let ceiling = MIN_CEILING_DB.max(live.peak_db);

        let target = curved(rms_db, ceiling);
        live.level += (target - live.level) * if target > live.level { ATTACK } else { RELEASE };
        live.levels.copy_within(1.., 0);
        live.levels[LEVEL_HISTORY - 1] = live.level as f32;

        let Live { tail, buffer, scratch, spectrum, .. } = live;
        for ((slot, sample), weight) in buffer.iter_mut().zip(tail.iter()).zip(&self.window) {
            *slot = Complex32::new(sample * weight, 0.0);
        }
        self.fft.process_with_scratch(buffer, scratch);
        let mut bands = [0f64; SPECTRUM_BANDS];
        for (bin, band) in buffer.iter().zip(&self.band_of_bin) {
            if let Some(band) = band {
                bands[*band] += f64::from(bin.norm_sqr() * self.scale);
            }
        }
        for (value, power) in spectrum.iter_mut().zip(bands) {
            let target = curved(db(power), ceiling) as f32;
            let rate = if target > *value { ATTACK as f32 } else { RELEASE as f32 };
            *value += (target - *value) * rate;
        }
    }
}
