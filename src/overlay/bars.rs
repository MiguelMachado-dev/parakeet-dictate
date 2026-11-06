//! Overlay renderer: a smoked-glass capsule whose mirrored bars carry every phase.

use std::f64::consts::TAU;
use std::sync::LazyLock;

use super::frame::{BEAT_S, OverlayFrame, Phase};
use super::{clip, interp};

type Rgb = [f32; 3];
type Bars = [f32; N_BARS];

const N_BARS: usize = 31;
const CENTER: f32 = (N_BARS as f32 - 1.0) / 2.0;
const PITCH: f64 = 6.0;
const BAR_R: f64 = 1.5;
const GLOW_MARGIN: f64 = 8.5;
const L_MAX: f64 = 14.5;
const HOPS_PER_BAR: f32 = 2.0;
const CAPSULE_W: f64 = 214.0;
const CAPSULE_H: f64 = 46.0;
const LATCH_W: f64 = 14.0;
const CY: f64 = 58.0;

const GLASS_TOP: Rgb = [0.105, 0.115, 0.145];
const GLASS_BOTTOM: Rgb = [0.045, 0.052, 0.068];
const GLASS_ALPHA: f32 = 0.9;

type Stop = (f64, (u8, u8, u8));

// Budgerigar plumage, yellow face to green body to blue tail: fresh sound at the centre, cooling as it ages outward.
// scripts/make_icon.py paints the app icon's bars with the same stops.
const PLUMAGE_STOPS: [Stop; 5] = [
    (0.0, (250, 255, 185)),
    (0.2, (205, 250, 110)),
    (0.47, (70, 228, 160)),
    (0.76, (38, 160, 205)),
    (1.0, (62, 84, 160)),
];
const IRIS: [Stop; 4] = [(0.0, (140, 92, 255)), (0.33, (225, 84, 255)), (0.66, (92, 140, 255)), (1.0, (140, 92, 255))];
const IDLE_C: Rgb = [0.5, 0.72, 0.8];
const SCAN: Rgb = [0.86, 0.93, 1.0];
const DONE_C: Rgb = [0.38, 0.94, 0.62];
const CANCEL_C: Rgb = [0.55, 0.57, 0.62];
const FAIL_C: Rgb = [1.0, 0.43, 0.43];
const LATCH_C: Rgb = [0.9, 0.92, 0.96];

static K: LazyLock<Bars> = LazyLock::new(|| std::array::from_fn(|i| (i as f32 - CENTER).abs()));
static K_NORM: LazyLock<Bars> = LazyLock::new(|| K.map(|k| k / CENTER));
static PLUMAGE: LazyLock<[Rgb; N_BARS]> = LazyLock::new(|| K_NORM.map(|t| ramp(&PLUMAGE_STOPS, t)));

fn ramp(stops: &[Stop], t: f32) -> Rgb {
    let t = f64::from(t);
    let channel = |stop: &Stop, ch: usize| {
        let (r, g, b) = stop.1;
        f64::from(f32::from([r, g, b][ch]) / 255.0)
    };
    let upper = stops.iter().position(|stop| t < stop.0).unwrap_or(stops.len());
    std::array::from_fn(|ch| {
        if upper == 0 {
            return channel(&stops[0], ch) as f32;
        }
        if upper == stops.len() {
            return channel(&stops[upper - 1], ch) as f32;
        }
        let (low, high) = (&stops[upper - 1], &stops[upper]);
        let slope = (channel(high, ch) - channel(low, ch)) / (high.0 - low.0);
        (slope * (t - low.0) + channel(low, ch)) as f32
    })
}

fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Glyph {
    Latch,
    Check,
    Alert,
}

/// What the phase asks of the strip. Per-bar arrays run left to right; lengths are logical px.
#[derive(Clone)]
struct Pose {
    halves: Bars,
    colors: [Rgb; N_BARS],
    alpha: Bars,
    aura: Bars,
    flare: f64,
    width: f64,
    pitch: f64,
    shift: f64,
    rim: Option<Rgb>,
    rim_strength: f64,
    glyph: Option<Glyph>,
    glyph_t: f64,
}

fn each(value: impl FnMut(usize) -> f32) -> Bars {
    std::array::from_fn(value)
}

fn recording(frame: &OverlayFrame<'_>) -> Pose {
    let levels: Vec<f32> = frame.levels.iter().rev().copied().collect();
    let (k, k_norm) = (&*K, &*K_NORM);
    let history = each(|i| interp(f64::from(k[i] * HOPS_PER_BAR), &levels) as f32);
    // Silence ripples outward the same way speech does, only a pixel tall.
    let omega = (TAU * 0.55 * frame.recording_elapsed) as f32;
    let breath = each(|i| 0.5 + 0.5 * (omega - k[i] * 0.55).sin());
    let lit = history.map(|value| clip(value * 2.5));
    let band_step = 17.0 / CENTER;
    let l_max = L_MAX as f32;
    Pose {
        halves: each(|i| {
            let voice = history[i].powf(1.7) * (1.0 - 0.5 * k_norm[i] * k_norm[i]);
            (1.2 * breath[i]).max(voice * l_max)
        }),
        colors: std::array::from_fn(|i| std::array::from_fn(|ch| IDLE_C[ch] + (PLUMAGE[i][ch] - IDLE_C[ch]) * lit[i])),
        alpha: each(|i| (1.0 - 0.45 * k_norm[i] * k_norm[i]) * (0.55 + 0.45 * breath[i].max(lit[i]))),
        // Speech energy sits in bands 3 to 20 (about 150 Hz to 5 kHz); the lowest of those breathe at the centre.
        aura: each(|i| {
            let band = interp(f64::from(3.0 + k[i] * band_step), frame.spectrum);
            (band * 1.8).clamp(0.0, 1.0) as f32
        }),
        flare: 0.6,
        width: 1.0,
        pitch: 1.0,
        shift: 0.0,
        rim: None,
        rim_strength: 0.0,
        glyph: frame.hands_free.then_some(Glyph::Latch),
        glyph_t: 0.0,
    }
}

fn transcribing(frame: &OverlayFrame<'_>) -> Pose {
    let t = frame.phase_elapsed;
    let settle = ease(t / 0.15) as f32;
    let head = (1.0 - (t / 0.55).rem_euclid(1.0)) as f32;
    let k_norm = &*K_NORM;
    let bump = each(|i| (-((k_norm[i] - head) / 0.16).powi(2)).exp());
    let start = recording(frame);
    Pose {
        halves: each(|i| start.halves[i] + (1.0 + 7.0 * bump[i] - start.halves[i]) * settle),
        colors: std::array::from_fn(|i| {
            std::array::from_fn(|ch| start.colors[i][ch] + (SCAN[ch] * (0.55 + 0.45 * bump[i]) - start.colors[i][ch]) * settle)
        }),
        alpha: each(|i| start.alpha[i] + (1.0 - start.alpha[i]) * settle),
        aura: each(|i| start.aura[i] * (1.0 - settle) + bump[i] * 0.6 * settle),
        flare: start.flare * (1.0 - f64::from(settle)),
        glyph: None,
        ..start
    }
}

fn polishing(frame: &OverlayFrame<'_>) -> Pose {
    let t = frame.phase_elapsed;
    let settle = ease(t / 0.25) as f32;
    let (k, k_norm) = (&*K, &*K_NORM);
    let (omega_a, omega_b) = ((TAU * 0.7 * t) as f32, (TAU * 0.27 * t) as f32);
    let a = each(|i| 0.5 + 0.5 * (omega_a - k[i] * 0.5).sin());
    let b = each(|i| 0.5 + 0.5 * (omega_b + k[i] * 0.23 + 1.1).sin());
    let working = each(|i| 1.5 + 9.0 * a[i] * (0.35 + 0.65 * b[i]) * (1.0 - 0.45 * k_norm[i] * k_norm[i]));
    let drift = (t * 0.35) as f32;
    let spread = (N_BARS as f32) * 1.2;
    Pose {
        halves: each(|i| 1.0 + (working[i] - 1.0) * settle),
        colors: std::array::from_fn(|i| {
            let color = ramp(&IRIS, (k[i] / spread - drift).rem_euclid(1.0));
            std::array::from_fn(|ch| SCAN[ch] + (color[ch] - SCAN[ch]) * settle)
        }),
        alpha: [1.0; N_BARS],
        aura: each(|i| (0.25 + 0.5 * a[i] * b[i]) * settle),
        flare: 0.0,
        width: 1.0,
        pitch: 1.0,
        shift: 0.0,
        rim: None,
        rim_strength: 0.0,
        glyph: None,
        glyph_t: 0.0,
    }
}

fn ending(frame: &OverlayFrame<'_>, color: Rgb, glyph: Option<Glyph>) -> Pose {
    let t = frame.phase_elapsed;
    let fall = ease(t / 0.14);
    let k = &*K;
    let alpha = each(|i| {
        let clear = if glyph.is_some() { f64::from(clip((k[i] - 3.0) / 2.0)) } else { 1.0 };
        ((clear + (1.0 - clear) * (1.0 - fall)) * (1.0 - 0.4 * fall)) as f32
    });
    Pose {
        halves: [(1.0 - fall) as f32; N_BARS],
        colors: [color; N_BARS],
        alpha,
        aura: [0.0; N_BARS],
        flare: 0.0,
        width: 1.0,
        pitch: 1.0,
        shift: 0.0,
        rim: Some(color),
        rim_strength: (-t * 6.0).exp(),
        glyph,
        glyph_t: t,
    }
}

fn done(frame: &OverlayFrame<'_>) -> Pose {
    let pose = ending(frame, DONE_C, Some(Glyph::Check));
    let k_norm = &*K_NORM;
    let front = (frame.phase_elapsed / 0.32) as f32;
    let wave = each(|i| (-((k_norm[i] - front) / 0.14).powi(2)).exp() * (1.0 - k_norm[i]).powf(1.5));
    Pose { halves: each(|i| pose.halves[i] + 3.0 * wave[i]), aura: wave.map(|value| value * 0.8), ..pose }
}

fn cancelled(frame: &OverlayFrame<'_>) -> Pose {
    let fold = ease(frame.phase_elapsed / BEAT_S);
    let pose = ending(frame, CANCEL_C, None);
    Pose { pitch: 1.0 - 0.55 * fold, width: 1.0 - 0.4 * fold, rim_strength: 0.0, ..pose }
}

fn failed(frame: &OverlayFrame<'_>) -> Pose {
    let t = frame.phase_elapsed;
    let shift = 4.0 * (-t * 7.0).exp() * (TAU * 7.5 * t).sin();
    Pose { shift, ..ending(frame, FAIL_C, Some(Glyph::Alert)) }
}

fn pose_of(frame: &OverlayFrame<'_>) -> Pose {
    match frame.phase {
        Phase::Recording => recording(frame),
        Phase::Transcribing => transcribing(frame),
        Phase::Polishing => polishing(frame),
        Phase::Done => done(frame),
        Phase::Cancelled => cancelled(frame),
        Phase::Failed => failed(frame),
    }
}

/// Composite straight `rgb` at `alpha` over the premultiplied pixel `c` and `a`.
fn over(c: &mut [f32], a: &mut f32, rgb: Rgb, alpha: f32) {
    let keep = 1.0 - alpha;
    for ch in 0..3 {
        c[ch] = c[ch] * keep + rgb[ch] * alpha;
    }
    *a = *a * keep + alpha;
}

fn segment(px: f32, py: f32, a: (f64, f64), b: (f64, f64)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (ax, ay) = (a.0 as f32, a.1 as f32);
    let (dx32, dy32) = (dx as f32, dy as f32);
    let h = clip(((px - ax) * dx32 + (py - ay) * dy32) / (dx * dx + dy * dy).max(1e-6) as f32);
    (px - ax - dx32 * h).hypot(py - ay - dy32 * h)
}

/// Signed distance in device pixels, and colour, of a glyph centred on (0, 0).
fn glyph_at(kind: Glyph, t: f64, px: f32, py: f32, s: f64) -> (f32, Rgb) {
    let unit = s as f32;
    match kind {
        Glyph::Latch => {
            let (qx, qy) = (px.abs() - 3.3 * unit, (py - 2.0 * unit).abs() - 2.3 * unit);
            let body = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - 1.0 * unit;
            let shackle = if py < -2.0 * unit {
                (px.hypot(py + 2.0 * unit) - 2.5 * unit).abs()
            } else {
                segment(px.abs(), py, (2.5 * s, -2.0 * s), (2.5 * s, -0.5 * s))
            } - 0.75 * unit;
            (body.min(shackle), LATCH_C)
        }
        Glyph::Check => {
            let drawn = ease((t - 0.04) / 0.22);
            let (a, b, c) = ((-7.0 * s, 0.4 * s), (-2.3 * s, 5.0 * s), (7.6 * s, -5.8 * s));
            let first = 0.36;
            let u = (drawn / first).min(1.0);
            let mut d = segment(px, py, a, (a.0 + (b.0 - a.0) * u, a.1 + (b.1 - a.1) * u));
            if drawn > first {
                let u = (drawn - first) / (1.0 - first);
                d = d.min(segment(px, py, b, (b.0 + (c.0 - b.0) * u, b.1 + (c.1 - b.1) * u)));
            }
            (d - 1.6 * unit, DONE_C)
        }
        Glyph::Alert => {
            let stem = segment(px, py, (0.0, -7.0 * s), (0.0, 1.6 * s)) - 1.6 * unit;
            let dot = px.hypot(py - 6.2 * unit) - 1.75 * unit;
            (stem.min(dot), FAIL_C)
        }
    }
}

/// Premultiplied colour, alpha and signed distance of the shadowed glass capsule.
struct Shell {
    c: Vec<f32>,
    a: Vec<f32>,
    d: Vec<f32>,
    rows: usize,
    cols: usize,
}

fn shell(qx2: &[f32], dy: &[f32], half_h: f64, s: f64, rim_rgb: Rgb, rim_strength: f64) -> Shell {
    let (rows, cols) = (dy.len(), qx2.len());
    let mut c = vec![0f32; rows * cols * 3];
    let mut a = vec![0f32; rows * cols];
    let mut d = vec![0f32; rows * cols];
    let (half, unit) = (half_h as f32, s as f32);
    let two_half = (2.0 * half_h) as f32;
    let drop = (4.0 * s) as f32;
    let (near, wide, tight) = ((2.0 * s) as f32, (13.0 * s) as f32, (3.0 * s) as f32);
    let ring = (0.75 * s) as f32;
    let outer_at = (0.5 * s) as f32;
    let strength = (0.55 * rim_strength) as f32;
    for row in 0..rows {
        let (dy_now, offset) = (dy[row] * dy[row], dy[row] - drop);
        let offset2 = offset * offset;
        let depth = clip((dy[row] + half) / two_half);
        let glass: Rgb = std::array::from_fn(|ch| GLASS_TOP[ch] + (GLASS_BOTTOM[ch] - GLASS_TOP[ch]) * depth);
        let top = clip(-dy[row] / half).powf(1.5);
        let light = 0.1 + 0.22 * top + strength;
        for col in 0..cols {
            let i = row * cols + col;
            let distance = (qx2[col] + dy_now).sqrt() - half;
            let shadow_d = (qx2[col] + offset2).sqrt() - half;
            let far = clip(1.0 - (shadow_d + near) / wide);
            let shadow = (0.34 * far * far).max(0.22 * clip(1.0 - (shadow_d - near) / tight));
            let cover = clip(0.5 - distance) * GLASS_ALPHA;
            let pixel = &mut c[i * 3..i * 3 + 3];
            for ch in 0..3 {
                pixel[ch] = glass[ch] * cover;
            }
            let mut alpha = cover + shadow * (1.0 - cover);
            // A light hairline just inside the edge holds the shape on dark backgrounds, a dark one just
            // outside holds it on white; the top of the light one doubles as the glass highlight.
            let inner = clip(1.0 - (distance + unit).abs() / ring) * light;
            let outer = clip(1.0 - (distance - outer_at).abs() / ring) * 0.35;
            let share = inner / (inner + outer).max(1e-6);
            over(pixel, &mut alpha, rim_rgb.map(|value| value * share), inner.max(outer));
            a[i] = alpha;
            d[i] = distance;
        }
    }
    Shell { c, a, d, rows, cols }
}

#[derive(Clone, Copy)]
struct Window {
    rows: (usize, usize),
    cols: (usize, usize),
}

/// Draw the mirrored bars, their bloom and the spectral aura into the premultiplied shell.
fn bars(pose: &Pose, shell: &mut Shell, box_of: Window, x: &[f32], y: &[f32], cx: f64, cy: f64, pitch: f64, s: f64) {
    let (cx32, cy32, pitch32) = (cx as f32, cy as f32, pitch as f32);
    let centers: Bars = each(|i| cx32 + (i as f32 - CENTER) * pitch32);
    let columns = &x[box_of.cols.0..box_of.cols.1];
    let lines = &y[box_of.rows.0..box_of.rows.1];
    let index: Vec<usize> = columns
        .iter()
        .map(|x| (((x - centers[0]) / pitch32).round_ties_even()).clamp(0.0, (N_BARS - 1) as f32) as usize)
        .collect();
    let dx2: Vec<f32> = columns.iter().zip(&index).map(|(x, i)| (x - centers[*i]).powi(2)).collect();
    let half: Vec<f32> = index.iter().map(|i| pose.halves[*i] * s as f32).collect();
    let alpha: Vec<f32> = index.iter().map(|i| pose.alpha[*i]).collect();

    // The aura reaches zero at the box edge so it never shows a seam.
    let margin = (GLOW_MARGIN * s) as f32;
    let mut xp = Vec::with_capacity(N_BARS + 2);
    xp.push(centers[0] - margin);
    xp.extend_from_slice(&centers);
    xp.push(centers[N_BARS - 1] + margin);
    let mut fp = Vec::with_capacity(N_BARS + 2);
    fp.push(0.0);
    fp.extend_from_slice(&pose.aura);
    fp.push(0.0);
    let amp: Vec<f32> = columns.iter().map(|x| aura_at(f64::from(*x), &xp, &fp)).collect();
    let spread: Vec<f32> = amp.iter().map(|amp| (5.0 + 12.0 * amp) * s as f32).collect();

    let inset = (-1.5 * s) as f32;
    let radius = (BAR_R * s) as f32;
    let tip = ((L_MAX + BAR_R) * s) as f32;
    let flare = pose.flare as f32;
    let bloom_step = (1.8 * s) as f32;
    for (row, line) in lines.iter().enumerate() {
        let dy = (line - cy32).abs();
        let along = clip(dy / tip);
        let white = flare * along.powf(2.5);
        let gain = (0.7 + 0.3 * along) * (1.0 - white);
        for (col, bar) in index.iter().enumerate() {
            let i = (row + box_of.rows.0) * shell.cols + col + box_of.cols.0;
            let interior = clip(inset - shell.d[i]);
            let reach = (dy - half[col]).max(0.0);
            let bar_d = (dx2[col] + reach * reach).sqrt() - radius;
            let bar_a = clip(0.5 - bar_d) * alpha[col] * interior;
            let keep = 1.0 - bar_a;
            let bloom = (-bar_d.max(0.0) / bloom_step).exp() * keep * interior * 0.3;
            let aura = (-(dy / spread[col]).powi(2)).exp() * (amp[col] * 0.32) * interior;
            let lit = bar_a + bloom;
            let light = gain * lit + aura;
            let pixel = &mut shell.c[i * 3..i * 3 + 3];
            for ch in 0..3 {
                pixel[ch] = pixel[ch] * keep + pose.colors[*bar][ch] * light + white * lit;
            }
            shell.a[i] = shell.a[i] * keep + bar_a;
        }
    }
}

/// `np.interp(x, xp, fp)` for ascending `xp`.
fn aura_at(x: f64, xp: &[f32], fp: &[f32]) -> f32 {
    let upper = xp.partition_point(|point| f64::from(*point) <= x);
    if upper == 0 {
        return fp[0];
    }
    if upper == xp.len() {
        return fp[fp.len() - 1];
    }
    let (x0, x1) = (f64::from(xp[upper - 1]), f64::from(xp[upper]));
    let (y0, y1) = (f64::from(fp[upper - 1]), f64::from(fp[upper]));
    ((y1 - y0) / (x1 - x0) * (x - x0) + y0) as f32
}

pub fn render(frame: &OverlayFrame<'_>, out: &mut [u8]) {
    let (w, h) = frame.size();
    out.fill(0);
    if frame.presence <= 0.0 {
        return;
    }
    let s = frame.scale;
    let pose = pose_of(frame);

    // A dot fades in, then stretches into the capsule; leaving plays it backwards.
    let p = frame.presence;
    let full_w = (CAPSULE_W + if frame.hands_free { 2.0 * LATCH_W } else { 0.0 }) * pose.width;
    let dot = CAPSULE_H * (0.5 + 0.5 * ease(p / 0.6));
    let half_h = dot * s / 2.0;
    let half_w = (dot + (full_w - dot) * ease((p - 0.15) / 0.85)) * s / 2.0;
    let cx = w as f64 / 2.0 + pose.shift * s;
    let cy = CY * s;

    let x0 = ((cx - half_w - 12.0 * s) as i64).max(0);
    let x1 = ((cx + half_w + 12.0 * s).ceil() as i64).min(w as i64);
    let y0 = ((cy - half_h - 8.0 * s) as i64).max(0);
    let y1 = ((cy + half_h + 16.0 * s).ceil() as i64).min(h as i64);
    let x: Vec<f32> = (x0..x1).map(|value| value as f32 + 0.5).collect();
    let y: Vec<f32> = (y0..y1).map(|value| value as f32 + 0.5).collect();
    let (cx32, cy32) = (cx as f32, cy as f32);
    let flat = (half_w - half_h) as f32;
    let qx2: Vec<f32> = x.iter().map(|x| ((x - cx32).abs() - flat).max(0.0).powi(2)).collect();
    let dy: Vec<f32> = y.iter().map(|y| y - cy32).collect();
    let mut shell = shell(&qx2, &dy, half_h, s, pose.rim.unwrap_or([1.0; 3]), pose.rim_strength);

    let window = |bx: f64, by: f64, rx: f64, ry: f64, shell: &Shell| {
        let clamp = |start: i64, end: i64, length: usize| {
            let start = start.max(0) as usize;
            (start.min(length), (end.max(0) as usize).min(length).max(start.min(length)))
        };
        Window {
            rows: clamp((by - ry) as i64 - y0, (by + ry).ceil() as i64 - y0, shell.rows),
            cols: clamp((bx - rx) as i64 - x0, (bx + rx).ceil() as i64 - x0, shell.cols),
        }
    };

    let pitch = PITCH * pose.pitch * s;
    let strip = window(cx, cy, f64::from(CENTER) * pitch + GLOW_MARGIN * s, (L_MAX + BAR_R + 6.5) * s, &shell);
    bars(&pose, &mut shell, strip, &x, &y, cx, cy, pitch, s);

    if let Some(glyph) = pose.glyph {
        let gx = if glyph == Glyph::Latch { cx - half_w + 17.5 * s } else { cx };
        let area = window(gx, cy, 14.0 * s, 14.0 * s, &shell);
        let (gx32, inset) = (gx as f32, (-1.5 * s) as f32);
        let glow_step = (2.5 * s) as f32;
        for row in area.rows.0..area.rows.1 {
            for col in area.cols.0..area.cols.1 {
                let (gd, ink) = glyph_at(glyph, pose.glyph_t, x[col] - gx32, y[row] - cy32, s);
                let i = row * shell.cols + col;
                let inside = clip(inset - shell.d[i]);
                let g_a = clip(0.5 - gd) * inside;
                let pixel = &mut shell.c[i * 3..i * 3 + 3];
                over(pixel, &mut shell.a[i], ink, g_a);
                if glyph != Glyph::Latch {
                    let glow = (-gd.max(0.0) / glow_step).exp() * (1.0 - g_a) * inside * 0.35;
                    for ch in 0..3 {
                        pixel[ch] += ink[ch] * glow;
                    }
                }
            }
        }
    }

    // Additive light can push premultiplied colour past alpha; clamp it, then unpremultiply.
    let gain = (255.0 * (p / 0.3).min(1.0)) as f32;
    for row in 0..shell.rows {
        let target = &mut out[((row + y0 as usize) * w + x0 as usize) * 4..][..shell.cols * 4];
        for (col, pixel) in target.chunks_exact_mut(4).enumerate() {
            let i = row * shell.cols + col;
            let alpha = (shell.a[i] * gain + 0.5) as u8;
            let divisor = u32::from(alpha);
            for ch in 0..3 {
                let value = (shell.c[i * 3 + ch].min(shell.a[i]) * gain + 0.5) as u8;
                pixel[ch] = if divisor == 0 || divisor == 255 { value } else { (255 * u32::from(value) / divisor).min(255) as u8 };
            }
            pixel[3] = alpha;
        }
    }
}
