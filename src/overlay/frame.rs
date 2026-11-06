//! What the overlay draws on each frame. A renderer is a pure function of an `OverlayFrame`.

// Logical pixels at 100% scale. The renderer draws everything inside it, glow and shadow included,
// so the window never resizes.
pub const CANVAS_SIZE: (usize, usize) = (400, 120);
pub const FPS: f64 = 60.0;
// How long the DONE, CANCELLED and FAILED beats play before the window fades out.
pub const BEAT_S: f64 = 0.45;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Recording,
    Transcribing,
    Polishing,
    Done,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayFrame<'a> {
    pub phase: Phase,
    pub phase_elapsed: f64,
    /// 0 is fully gone, 1 is fully shown. The window eases it on show and hide; the renderer
    /// decides what partial presence looks like.
    pub presence: f64,
    pub hands_free: bool,
    /// Seconds since the recording began, frozen once it stops.
    pub recording_elapsed: f64,
    /// `meter::LEVEL_HISTORY` values in 0..1, oldest first, one per meter hop.
    pub levels: &'a [f32],
    /// `meter::SPECTRUM_BANDS` values in 0..1, low to high frequency.
    pub spectrum: &'a [f32],
    pub scale: f64,
}

pub fn scaled(length: usize, scale: f64) -> usize {
    (length as f64 * scale).round_ties_even() as usize
}

impl OverlayFrame<'_> {
    pub fn size(&self) -> (usize, usize) {
        (scaled(CANVAS_SIZE.0, self.scale), scaled(CANVAS_SIZE.1, self.scale))
    }
}
