//! Plumage: voice ribbons in budgerigar colours, held in a night-blue capsule.

use std::f64::consts::{PI, TAU};

use super::clip;
use super::frame::{BEAT_S, OverlayFrame, Phase};
use super::interp;

type Rgb = [f32; 3];

// Logical pixels, multiplied by frame.scale.
const HALF_LEN: f64 = 134.0;
const RADIUS: f64 = 26.0;
const LIFT: f64 = 3.0;
const SHADOW_DROP: f64 = 4.0;
const SHADOW_REACH: f64 = 15.0;
const SHADOW_ALPHA: f32 = 0.45;
const HALO_REACH: f64 = 13.0;
const HALO_ALPHA: f64 = 0.55;
const CREST_ROOM: f64 = 5.0;
const CAP_REACH: f64 = 0.55;
const LOCK_ROOM: f64 = 16.0;
const GLOW_REACH: f64 = 7.0;

const MEMBRANE: f32 = 0.3;
const EDGE: f32 = 1.0;
const GLOW: f32 = 0.3;

// History hops a ripple takes to travel from the middle of the capsule to its ends.
const RIPPLE_HOPS: f64 = 34.0;
const RIPPLE_CORE: f64 = 0.14;
const SMOOTH: [f32; 5] = [1.0 / 9.0, 2.0 / 9.0, 3.0 / 9.0, 2.0 / 9.0, 1.0 / 9.0];

const fn channel(hex: u32, shift: u32) -> f32 {
    (((hex >> shift) & 0xFF) as f64 / 255.0) as f32
}

const fn rgb(hex: u32) -> Rgb {
    [channel(hex, 16), channel(hex, 8), channel(hex, 0)]
}

const NIGHT: [Rgb; 2] = [rgb(0x1a2234), rgb(0x090e18)];
const INDIGO: [Rgb; 2] = [rgb(0x251a4a), rgb(0x0d0920)];
const MOSS: [Rgb; 2] = [rgb(0x173a26), rgb(0x08170f)];
const WINE: [Rgb; 2] = [rgb(0x3a1820), rgb(0x170a0d)];
const GRAPHITE: [Rgb; 2] = [rgb(0x20232a), rgb(0x0d0f13)];
const RIM: Rgb = rgb(0xa9b8d6);
const WHITE: Rgb = rgb(0xffffff);
const COBALT: Rgb = rgb(0x3a6bff);
const SKY: Rgb = rgb(0x1fc8f0);
const GREEN: Rgb = rgb(0x79ec4f);
const LEMON: Rgb = rgb(0xffd84a);
const VIOLET: Rgb = rgb(0x8f63ff);
const ORCHID: Rgb = rgb(0xe46cff);
const CORAL: Rgb = rgb(0xff6a5c);
const ASH: Rgb = rgb(0x9aa3b2);
const LOCK_INK: Rgb = rgb(0xdfe6f5);
const SPARKLE_INK: Rgb = rgb(0xf1e9ff);
const CHECK_INK: Rgb = rgb(0xb6ff8f);
const PLUMAGE: [Rgb; 4] = [COBALT, SKY, GREEN, LEMON];
const THINKING: [Rgb; 4] = [VIOLET, SKY, ORCHID, COBALT];

// Per ribbon: spatial cycles across the span, phase speed (rad/s), phase offset, twist cycles.
const CYCLES: [f32; 4] = [1.25, 1.7, 2.2, 2.85];
const SPEED: [f32; 4] = [2.1, -2.7, 3.3, -4.1];
const OFFSET: [f32; 4] = [0.0, 2.1, 4.0, 5.5];
const TWIST: [f32; 4] = [0.9, 1.3, 1.1, 1.6];
const QUARTER: f32 = (PI / 2.0) as f32;
const QUARTERS: [f32; 4] = [0.0, QUARTER, 2.0 * QUARTER, 3.0 * QUARTER];
const PI32: f32 = PI as f32;

fn mix_rgb(a: Rgb, b: Rgb, m: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * m, a[1] + (b[1] - a[1]) * m, a[2] + (b[2] - a[2]) * m]
}

/// Four ribbons sampled at each column of the capsule, and the light and body around them.
/// `center`, `half` and `gain` hold 4 rows of X columns.
struct Look {
    center: Vec<f32>,
    half: Vec<f32>,
    gain: Vec<f32>,
    color: [Rgb; 4],
    aura: [Vec<f32>; 3],
    body: [Rgb; 2],
}

impl Look {
    fn mix(self, other: Look, m: f64) -> Look {
        if m <= 0.0 {
            return self;
        }
        if m >= 1.0 {
            return other;
        }
        let m = m as f32;
        let lerp = |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(a, b)| a + (b - a) * m).collect() };
        let mix_rows = |a: &[Rgb; 4], b: &[Rgb; 4]| -> [Rgb; 4] { std::array::from_fn(|r| mix_rgb(a[r], b[r], m)) };
        Look {
            center: lerp(&self.center, &other.center),
            half: lerp(&self.half, &other.half),
            gain: lerp(&self.gain, &other.gain),
            color: mix_rows(&self.color, &other.color),
            aura: std::array::from_fn(|ch| lerp(&self.aura[ch], &other.aura[ch])),
            body: [mix_rgb(self.body[0], other.body[0], m), mix_rgb(self.body[1], other.body[1], m)],
        }
    }
}

struct Stage<'a> {
    frame: &'a OverlayFrame<'a>,
    /// Column position along the ribbon span, -1..1.
    u: &'a [f32],
    /// 1 mid-span, 0 at the span ends.
    taper: &'a [f32],
    /// Px a crest may travel from the midline.
    reach: f64,
    s: f64,
}

impl Stage<'_> {
    fn t(&self) -> f64 {
        self.frame.phase_elapsed
    }

    fn taper_powers(&self, power: f32) -> Vec<f32> {
        self.taper.iter().map(|taper| taper.powf(power)).collect()
    }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let x = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The level history laid out from the middle (now) to the span ends (RIPPLE_HOPS ago).
fn ripple(levels: &[f32], u: &[f32]) -> Vec<f32> {
    let (first, last) = (levels[0], levels[levels.len() - 1]);
    let mut padded = Vec::with_capacity(levels.len() + 4);
    padded.extend([first, first]);
    padded.extend_from_slice(levels);
    padded.extend([last, last]);
    let smooth: Vec<f32> =
        padded.windows(5).map(|window| window.iter().zip(SMOOTH).map(|(value, weight)| value * weight).sum()).collect();
    // Hyperbolic rather than |u|, so a fresh onset swells as a rounded crest instead of a spike.
    let core = RIPPLE_CORE as f32;
    let core_squared = (RIPPLE_CORE * RIPPLE_CORE) as f32;
    let span = (1.0 + RIPPLE_CORE * RIPPLE_CORE).sqrt() - RIPPLE_CORE;
    let newest = (smooth.len() - 1) as f64;
    u.iter()
        .map(|u| {
            let away = f64::from((u * u + core_squared).sqrt() - core) / span;
            interp(newest - away * RIPPLE_HOPS, &smooth) as f32
        })
        .collect()
}

fn voice(st: &Stage<'_>) -> Look {
    let f = st.frame;
    let n = st.u.len();
    let energy: [f32; 4] = std::array::from_fn(|r| f.spectrum[r * 6..(r + 1) * 6].iter().sum::<f32>() / 6.0);
    let peak = energy.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let divisor = f64::from(peak).max(0.05) as f32;
    let share = energy.map(|value| value / divisor);
    let total = f64::from(energy.iter().sum::<f32>()) + 0.04;
    let tint: Rgb = std::array::from_fn(|ch| {
        let lit: f32 = (0..4).map(|r| energy[r] * PLUMAGE[r][ch]).sum();
        (lit + 0.04 * SKY[ch]) / total as f32
    });
    let ripple = ripple(f.levels, st.u);
    let t = f.recording_elapsed;
    let omega = (TAU * t / 2.6) as f32;
    let breathe: [f32; 4] = std::array::from_fn(|r| 0.85 + 0.15 * (omega + OFFSET[r]).sin());
    let (t32, slow, turn) = (t as f32, (1.7 * t) as f32, (1.3 * t) as f32);
    let (t07, t06) = (st.taper_powers(0.7), st.taper_powers(0.6));
    let reach = st.reach as f32;
    let s = st.s as f32;
    let mut center = vec![0f32; 4 * n];
    let mut half = vec![0f32; 4 * n];
    let mut gain = vec![0f32; 4 * n];
    for r in 0..4 {
        let cycles = PI32 * CYCLES[r];
        let twisting = PI32 * TWIST[r];
        for x in 0..n {
            let u = st.u[x];
            let drive = ripple[x] * (0.55 + 0.45 * share[r]);
            let phase = cycles * u + SPEED[r] * t32 + OFFSET[r];
            let wave = 0.8 * phase.sin() + 0.2 * (0.55 * phase - slow).sin();
            let amp = reach * st.taper[x] * (0.3 * breathe[r] + 1.1 * drive).min(1.0);
            let twist = (twisting * u - turn + OFFSET[r]).cos().abs();
            let i = r * n + x;
            center[i] = amp * wave;
            half[i] = s * (0.5 + (0.7 + 5.0 * drive) * twist) * t07[x];
            gain[i] = (0.6 + 1.1 * drive) * breathe[r] * t06[x];
        }
    }
    let recent = &f.levels[f.levels.len() - 3..];
    let loud = (0.05 + 0.4 * f64::from(recent.iter().sum::<f32>() / 3.0)) as f32;
    let aura = std::array::from_fn(|ch| st.taper.iter().map(|taper| tint[ch] * (loud * taper.powf(1.5))).collect());
    Look { center, half, gain, color: PLUMAGE, aura, body: NIGHT }
}

fn rope(st: &Stage<'_>) -> Look {
    let n = st.u.len();
    let t = st.t();
    let spin = (16.0 * t) as f32;
    let sweep = (-1.15 + 2.55 * (t / 0.5).rem_euclid(1.0)) as f32;
    let shine: Vec<f32> = st.u.iter().map(|u| (-((u - sweep) / 0.16).powi(2)).exp()).collect();
    let (t07, t06) = (st.taper_powers(0.7), st.taper_powers(0.6));
    let t12 = st.taper_powers(1.2);
    let reach = (0.16 * st.reach) as f32;
    let s = st.s as f32;
    let mut center = vec![0f32; 4 * n];
    let mut half = vec![0f32; 4 * n];
    let mut gain = vec![0f32; 4 * n];
    let wave = PI32 * 3.0;
    for r in 0..4 {
        for x in 0..n {
            let phase = wave * st.u[x] - spin + QUARTERS[r];
            let i = r * n + x;
            center[i] = reach * t12[x] * phase.sin();
            half[i] = s * (0.3 + 0.6 * phase.cos().abs()) * t07[x];
            gain[i] = (0.6 + 2.2 * shine[x]) * t06[x];
        }
    }
    let tint = mix_rgb(SKY, WHITE, 0.5);
    let aura = std::array::from_fn(|ch| (0..n).map(|x| tint[ch] * ((0.06 + 0.3 * shine[x]) * st.taper[x])).collect());
    Look { center, half, gain, color: PLUMAGE.map(|color| mix_rgb(color, WHITE, 0.5)), aura, body: NIGHT }
}

fn transcribing(st: &Stage<'_>) -> Look {
    voice(st).mix(rope(st), smoothstep(0.0, 0.08, st.t()))
}

fn polishing(st: &Stage<'_>) -> Look {
    const SIGN: [f32; 4] = [1.0, -1.0, 1.0, -1.0];
    const LAG: [f32; 4] = [0.0, 0.0, QUARTER, QUARTER];
    const SIZE: [f32; 4] = [1.0, 1.0, 0.55, 0.55];
    let n = st.u.len();
    let t = st.t();
    let breath = (TAU * t / 1.9).sin();
    let (t07, t06) = (st.taper_powers(0.7), st.taper_powers(0.6));
    let t13 = st.taper_powers(1.3);
    let sway = ((0.5 + 0.16 * breath) * st.reach) as f32;
    let s = st.s as f32;
    let (wave, shimmer_wave) = (PI32 * 1.6, 1.2 * PI32);
    let (spin, shimmer_spin) = ((2.4 * t) as f32, (1.7 * t) as f32);
    let along = (t / 1.7) as f32;
    let mut center = vec![0f32; 4 * n];
    let mut half = vec![0f32; 4 * n];
    let mut gain = vec![0f32; 4 * n];
    for r in 0..4 {
        let spark_at = -1.3 + 2.6 * (along + QUARTERS[r] * 0.17).rem_euclid(1.0);
        for x in 0..n {
            let u = st.u[x];
            let phase = wave * u - spin + LAG[r];
            let shimmer = 0.5 + 0.5 * (shimmer_wave * u - shimmer_spin + QUARTERS[r]).cos();
            let spark = (-((u - spark_at) / 0.05).powi(2)).exp();
            let i = r * n + x;
            center[i] = SIGN[r] * SIZE[r] * sway * t13[x] * phase.sin();
            half[i] = s * (0.5 + 3.2 * phase.cos().abs()) * t07[x];
            gain[i] = (shimmer + 1.6 * spark) * t06[x];
        }
    }
    let light = (0.14 + 0.06 * breath) as f32;
    let aura = std::array::from_fn(|ch| st.taper.iter().map(|taper| VIOLET[ch] * (light * taper.powf(1.5))).collect());
    let helix = Look { center, half, gain, color: THINKING, aura, body: INDIGO };
    rope(st).mix(helix, smoothstep(0.0, 0.3, t))
}

fn line(st: &Stage<'_>, color: Rgb, body: [Rgb; 2], span: f64, flash: f64, droop: f64) -> Look {
    let n = st.u.len();
    let room = span.max(1e-3) as f32;
    let kept: Vec<f32> = st.u.iter().map(|u| clip(1.0 - u.abs() / room)).collect();
    let taper: Vec<f32> = st.taper.iter().zip(&kept).map(|(taper, kept)| taper.min(kept * 3.0)).collect();
    let bend = (droop * st.reach) as f32;
    let s = (st.s * 0.9) as f32;
    let flash32 = flash as f32;
    let row = |value: &dyn Fn(usize) -> f32| -> Vec<f32> { (0..4).flat_map(|_| (0..n).map(value)).collect() };
    let center = row(&|x| bend * (1.0 - st.u[x] * st.u[x]) * st.taper[x]);
    let half = row(&|x| s * taper[x].powf(0.7));
    let gain = row(&|x| flash32 * taper[x].powf(0.6));
    let light = (0.12 * flash) as f32;
    let aura = std::array::from_fn(|ch| st.taper.iter().map(|taper| color[ch] * (light * taper)).collect());
    Look { center, half, gain, color: [color; 4], aura, body }
}

fn done(st: &Stage<'_>) -> Look {
    let t = st.t().min(BEAT_S);
    let flash = 0.9 + 1.4 * (-((t - 0.08) / 0.07).powi(2)).exp();
    let closing = line(st, mix_rgb(GREEN, WHITE, 0.25), MOSS, 1.0 - smoothstep(0.04, 0.17, t), flash, 0.0);
    rope(st).mix(closing, smoothstep(0.0, 0.08, t))
}

fn cancelled(st: &Stage<'_>) -> Look {
    let t = st.t().min(BEAT_S);
    let closing = line(st, ASH, GRAPHITE, 1.0 - smoothstep(0.12, 0.42, t), 0.7, 0.0);
    voice(st).mix(closing, smoothstep(0.0, 0.16, t))
}

fn failed(st: &Stage<'_>) -> Look {
    let t = st.t().min(BEAT_S);
    let closing = line(st, CORAL, WINE, 1.0, 1.1, 0.3 * smoothstep(0.05, 0.3, t));
    rope(st).mix(closing, smoothstep(0.0, 0.1, t))
}

fn look_of(st: &Stage<'_>) -> Look {
    match st.frame.phase {
        Phase::Recording => voice(st),
        Phase::Transcribing => transcribing(st),
        Phase::Polishing => polishing(st),
        Phase::Done => done(st),
        Phase::Cancelled => cancelled(st),
        Phase::Failed => failed(st),
    }
}

/// Add the four ribbons' tone-mapped light to `band`, 3 interleaved channels over `ys` rows of X columns.
fn add_ribbons(look: &Look, band: &mut [f32], ys: &[f32], s: f64, content: f32) {
    let n = look.center.len() / 4;
    let mut flatten = vec![0f32; 4 * n];
    for r in 0..4 {
        let c = &look.center[r * n..(r + 1) * n];
        for x in 0..n {
            let (from, to) = (x.clamp(1, n - 2) - 1, x.clamp(1, n - 2) + 1);
            let slope = (c[to] - c[from]) * 0.5;
            flatten[r * n + x] = 1.0 / (1.0 + slope * slope).sqrt();
        }
    }
    let glow_step = (-1.0 / (s * GLOW_REACH)) as f32;
    for (row, pixels) in ys.iter().zip(band.chunks_exact_mut(3 * n)) {
        for (x, pixel) in pixels.chunks_exact_mut(3).enumerate() {
            let mut light = [0f32; 3];
            for r in 0..4 {
                let i = r * n + x;
                let edge = (row - look.center[i]).abs() * flatten[i] - look.half[i];
                let mut acc = clip(0.5 - edge) * MEMBRANE;
                acc += (1.0 - edge.abs()).max(0.0) * EDGE;
                let mut tail = clip(edge * glow_step + 1.0);
                tail *= tail;
                tail *= tail;
                acc += tail * GLOW;
                acc *= look.gain[i];
                for ch in 0..3 {
                    light[ch] += look.color[r][ch] * acc;
                }
            }
            for ch in 0..3 {
                pixel[ch] += (1.0 - (-light[ch]).exp()) * content * (1.0 - pixel[ch]);
            }
        }
    }
}

fn segment(px: f32, py: f32, a: (f64, f64), b: (f64, f64)) -> f32 {
    let (bax, bay) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32);
    let (pax, pay) = (px - a.0 as f32, py - a.1 as f32);
    let length = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).max(1e-9) as f32;
    let h = clip((pax * bax + pay * bay) / length);
    (pax - bax * h).hypot(pay - bay * h)
}

fn lock_sdf(px: f32, py: f32) -> f32 {
    let (qx, qy) = (px.abs() - 3.3, (py - 2.6).abs() - 2.3);
    let body = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - 1.2;
    let arc = (px.hypot(py + 0.6) - 2.8).abs() - 0.85;
    let legs = ((px.abs() - 2.8).abs() - 0.85).max(py - 2.0);
    body.min(if py < -0.6 { arc } else { legs })
}

fn sparkle_sdf(px: f32, py: f32, size: f64, turn: f64) -> f32 {
    let (c, s) = (turn.cos() as f32, turn.sin() as f32);
    let (x, y) = ((c * px - s * py).abs(), (s * px + c * py).abs());
    let thin = size * 0.3;
    let norm = (size * thin / size.hypot(thin)) as f32;
    let (size, thin) = (size as f32, thin as f32);
    ((x / size + y / thin - 1.0) * norm).min((x / thin + y / size - 1.0) * norm)
}

fn check_sdf(px: f32, py: f32, progress: f64) -> f32 {
    const PTS: [(f64, f64); 3] = [(-7.6, 0.4), (-2.4, 5.6), (8.0, -5.6)];
    let lengths = [(PTS[1].0 - PTS[0].0).hypot(PTS[1].1 - PTS[0].1), (PTS[2].0 - PTS[1].0).hypot(PTS[2].1 - PTS[1].1)];
    let mut left = progress * (lengths[0] + lengths[1]);
    let mut d = f32::INFINITY;
    for (index, length) in lengths.into_iter().enumerate() {
        let k = (left / length).min(1.0);
        if k <= 0.0 {
            break;
        }
        let (a, b) = (PTS[index], PTS[index + 1]);
        d = d.min(segment(px, py, a, (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k)));
        left -= length;
    }
    d - 1.9
}

/// How present the hands-free lock is: fully while recording, fading out of the phases that follow it.
fn latched(frame: &OverlayFrame<'_>) -> f64 {
    if !frame.hands_free {
        return 0.0;
    }
    match frame.phase {
        Phase::Recording => 1.0,
        // How long the lock lingers into each phase that can directly follow a latched recording.
        Phase::Transcribing => 1.0 - smoothstep(0.0, 0.08, frame.phase_elapsed),
        Phase::Cancelled => 1.0 - smoothstep(0.0, 0.16, frame.phase_elapsed),
        _ => 0.0,
    }
}

#[derive(Clone, Copy)]
enum Mark {
    Lock,
    Sparkle { size: f64, turn: f64 },
    Check { progress: f64 },
}

struct Glyph {
    mark: Mark,
    ink: Rgb,
    left: bool,
    strength: f64,
}

/// The phase's glyph: an SDF in logical px, its colour, where it sits, and its opacity.
fn glyph_of(frame: &OverlayFrame<'_>) -> Option<Glyph> {
    let t = frame.phase_elapsed;
    let latch = latched(frame);
    if latch > 0.0 {
        return Some(Glyph { mark: Mark::Lock, ink: LOCK_INK, left: true, strength: latch });
    }
    if frame.phase == Phase::Polishing {
        let size = 6.0 * (1.0 + 0.2 * (TAU * t / 0.95).sin()) * smoothstep(0.05, 0.35, t).max(0.05);
        let turn = 0.35 * (TAU * t / 3.1).sin();
        return Some(Glyph { mark: Mark::Sparkle { size, turn }, ink: SPARKLE_INK, left: true, strength: 1.0 });
    }
    if frame.phase == Phase::Done {
        let progress = smoothstep(0.14, 0.32, t.min(BEAT_S));
        if progress > 0.0 {
            return Some(Glyph { mark: Mark::Check { progress }, ink: CHECK_INK, left: false, strength: 1.0 });
        }
    }
    None
}

pub fn render(frame: &OverlayFrame<'_>, out: &mut [u8]) {
    let (w, h) = frame.size();
    out.fill(0);
    if frame.presence <= 0.0 {
        return;
    }
    let (s, p) = (frame.scale, frame.presence);
    let radius = s * RADIUS * (0.4 + 0.6 * smoothstep(0.0, 0.5, p));
    let half_len = s * HALF_LEN * smoothstep(0.15, 1.0, p).powf(0.8);
    let mut shake = 0.0;
    if frame.phase == Phase::Failed {
        let t = frame.phase_elapsed.min(BEAT_S);
        shake = s * 5.0 * (TAU * 11.0 * t).sin() * (-t / 0.12).exp();
    }
    let (cx, cy) = (w as f64 / 2.0 + shake, h as f64 / 2.0 - s * LIFT);

    let reach = s * SHADOW_REACH.max(HALO_REACH);
    let ya = ((cy - radius - reach) as i64 - 1).max(0);
    let yb = ((cy + radius + s * SHADOW_DROP + reach) as i64 + 2).min(h as i64);
    let xa = ((cx - half_len - radius - reach) as i64 - 1).max(0);
    let xb = ((cx + half_len + radius + reach) as i64 + 2).min(w as i64);
    let (rows, cols) = ((yb - ya) as usize, (xb - xa) as usize);
    let (cx32, cy32, radius32, half_len32) = (cx as f32, cy as f32, radius as f32, half_len as f32);
    let xs: Vec<f32> = (xa..xb).map(|x| x as f32 + 0.5).collect();
    let dy: Vec<f32> = (ya..yb).map(|y| (y as f32 + 0.5) - cy32).collect();
    let dx2: Vec<f32> = xs
        .iter()
        .map(|x| {
            let dx = ((x - cx32).abs() - half_len32).max(0.0);
            dx * dx
        })
        .collect();
    let drop = (s * SHADOW_DROP) as f32;
    let shadow_step = (1.0 / (s * SHADOW_REACH)) as f32;
    let mut d = vec![0f32; rows * cols];
    let mut cover = vec![0f32; rows * cols];
    let mut under = vec![0f32; rows * cols];
    for row in 0..rows {
        let (dy_now, dys) = (dy[row] * dy[row], dy[row] - drop);
        let dys2 = dys * dys;
        for col in 0..cols {
            let i = row * cols + col;
            d[i] = (dx2[col] + dy_now).sqrt() - radius32;
            cover[i] = clip(0.5 - d[i]);
            let fall = clip(1.0 - ((dx2[col] + dys2).sqrt() - radius32) * shadow_step);
            under[i] = SHADOW_ALPHA * fall * fall;
        }
    }

    let ra = ((cy - radius) as i64 - 1 - ya).max(0) as usize;
    let rb = ((cy + radius) as i64 + 2 - ya).min(yb - ya) as usize;
    let ca = ((cx - half_len - radius) as i64 - 1 - xa).max(0) as usize;
    let cb = ((cx + half_len + radius) as i64 + 2 - xa).min(xb - xa) as usize;
    let box_dy = &dy[ra..rb];
    let box_xs = &xs[ca..cb];
    let (box_rows, box_cols) = (rb - ra, cb - ca);

    let left = cx - half_len - CAP_REACH * radius + s * LOCK_ROOM * latched(frame);
    let right = cx + half_len + CAP_REACH * radius;
    let (mid, span) = ((left + right) / 2.0, ((right - left) / 2.0).max(1.0));
    let (mid32, span32) = (mid as f32, span as f32);
    let u: Vec<f32> = box_xs.iter().map(|x| (x - mid32) / span32).collect();
    let taper: Vec<f32> = u.iter().map(|u| clip(1.0 - u * u)).collect();
    let stage = Stage { frame, u: &u, taper: &taper, reach: (radius - s * CREST_ROOM).max(1.0), s };
    let look = look_of(&stage);
    let content = smoothstep(0.05, 0.5, p);
    let content32 = content as f32;

    let halo_light: Rgb = std::array::from_fn(|ch| look.aura[ch].iter().sum::<f32>() / box_cols as f32 * content32);
    let halo_peak = f64::from(halo_light.iter().copied().fold(f32::NEG_INFINITY, f32::max));
    let mut halo: Option<Vec<f32>> = None;
    if halo_peak > 1e-3 {
        let step = (1.0 / (s * HALO_REACH)) as f32;
        let strength = (HALO_ALPHA * (halo_peak * 3.0).min(1.0)) as f32;
        let plane: Vec<f32> = d
            .iter()
            .map(|d| {
                let ring = clip(1.0 - d * step);
                strength * ring * ring * ring
            })
            .collect();
        for (under, halo) in under.iter_mut().zip(&plane) {
            *under = halo + *under * (1.0 - halo);
        }
        halo = Some(plane);
    }
    let alpha: Vec<f32> = cover.iter().zip(&under).map(|(cover, under)| cover + under * (1.0 - cover)).collect();

    let two_radius = (2.0 * radius) as f32;
    let quad = (1.6 / radius) as f32;
    let mut rgb_box = vec![0f32; box_rows * box_cols * 3];
    for row in 0..box_rows {
        let v = clip((box_dy[row] + radius32) / two_radius);
        let grad: Rgb = std::array::from_fn(|ch| look.body[0][ch] + (look.body[1][ch] - look.body[0][ch]) * v);
        let lit = 0.14 + 0.4 * (1.0 - v) * (1.0 - v);
        let q = box_dy[row] * quad;
        let falloff = 1.0 / (1.0 + q * q);
        for col in 0..box_cols {
            let distance = d[(row + ra) * cols + col + ca];
            let rim = clip(1.0 - (distance + 1.0).abs()) * lit;
            let at = (row * box_cols + col) * 3;
            for ch in 0..3 {
                let body = grad[ch] + (RIM[ch] - grad[ch]) * rim;
                let aura = 1.0 - (-(look.aura[ch][col] * content32) * falloff).exp();
                rgb_box[at + ch] = body + aura * (1.0 - body);
            }
        }
    }

    let furthest = look
        .center
        .iter()
        .zip(&look.half)
        .map(|(center, half)| center.abs() + half)
        .fold(f32::NEG_INFINITY, f32::max);
    let extent = (f64::from(furthest) + s * (GLOW_REACH + 1.0)) as f32;
    let first = box_dy.iter().position(|dy| dy.abs() <= extent);
    let last = box_dy.iter().rposition(|dy| dy.abs() <= extent);
    if let (Some(first), Some(last)) = (first, last)
        && content > 0.0
    {
        let band = &mut rgb_box[first * box_cols * 3..(last + 1) * box_cols * 3];
        add_ribbons(&look, band, &box_dy[first..=last], s, content32);
    }

    if let Some(glyph) = glyph_of(frame) {
        let gx = if glyph.left { cx - half_len } else { cx } as f32;
        let near = (s * 12.0) as f32;
        let gr: Vec<usize> = (0..box_rows).filter(|row| box_dy[*row].abs() < near).collect();
        let gc: Vec<usize> = (0..box_cols).filter(|col| (box_xs[*col] - gx).abs() < near).collect();
        if let (Some(&r0), Some(&r1), Some(&c0), Some(&c1)) = (gr.first(), gr.last(), gc.first(), gc.last()) {
            let s32 = s as f32;
            let opacity = (content * glyph.strength) as f32;
            for row in r0..=r1 {
                let py = box_dy[row] / s32;
                for col in c0..=c1 {
                    let px = (box_xs[col] - gx) / s32;
                    let distance = match glyph.mark {
                        Mark::Lock => lock_sdf(px, py),
                        Mark::Sparkle { size, turn } => sparkle_sdf(px, py, size, turn),
                        Mark::Check { progress } => check_sdf(px, py, progress),
                    };
                    let g = clip(0.5 - distance * s32) * opacity;
                    let at = (row * box_cols + col) * 3;
                    for ch in 0..3 {
                        rgb_box[at + ch] += (glyph.ink[ch] - rgb_box[at + ch]) * g;
                    }
                }
            }
        }
    }

    // Straight alpha: un-premultiply the body and the halo spill by the combined coverage.
    let fade = (255.0 * smoothstep(0.0, 0.3, p)) as f32;
    let spill: Rgb = if halo.is_some() { halo_light.map(|light| light / halo_peak as f32) } else { [0.0; 3] };
    for row in 0..rows {
        let target = &mut out[((row + ya as usize) * w + xa as usize) * 4..][..cols * 4];
        for (col, pixel) in target.chunks_exact_mut(4).enumerate() {
            let i = row * cols + col;
            let inv = 255.0 / alpha[i].max(1e-6);
            let glow = halo.as_ref().map_or(0.0, |halo| halo[i] * (1.0 - cover[i]) * inv);
            let inside = row >= ra && row < rb && col >= ca && col < cb;
            for ch in 0..3 {
                let mut plane = glow * spill[ch];
                if inside {
                    let at = ((row - ra) * box_cols + (col - ca)) * 3 + ch;
                    plane += clip(rgb_box[at]) * (cover[i] * inv);
                }
                pixel[ch] = (plane + 0.5) as u8;
            }
            pixel[3] = (alpha[i] * fade + 0.5) as u8;
        }
    }
}
