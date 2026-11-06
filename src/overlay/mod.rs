pub mod active;
pub mod bars;
pub mod fluid;
pub mod frame;
pub mod scene;
pub mod window;

use frame::OverlayFrame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Fluid,
    Bars,
}

impl Style {
    /// The style a saved overlay choice names. "off" names none.
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "fluid" => Some(Self::Fluid),
            "bars" => Some(Self::Bars),
            _ => None,
        }
    }

    /// Draw `frame` as straight-alpha RGBA, top-down, into `out` of `width * height * 4` bytes.
    pub fn render(self, frame: &OverlayFrame<'_>, out: &mut [u8]) {
        match self {
            Self::Fluid => fluid::render(frame, out),
            Self::Bars => bars::render(frame, out),
        }
    }
}

/// `fp` sampled at the fractional index `x`, held at both ends: `np.interp` over `arange(len(fp))`.
pub(crate) fn interp(x: f64, fp: &[f32]) -> f64 {
    let last = fp.len() - 1;
    if x <= 0.0 {
        return f64::from(fp[0]);
    }
    if x >= last as f64 {
        return f64::from(fp[last]);
    }
    let at = x.floor();
    let index = at as usize;
    let (low, high) = (f64::from(fp[index]), f64::from(fp[index + 1]));
    (high - low) * (x - at) + low
}

pub(crate) fn clip(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Straight-alpha RGBA to the premultiplied BGRA that `UpdateLayeredWindow` blends with `AC_SRC_ALPHA`.
pub fn premultiplied_bgra(rgba: &[u8], out: &mut [u8]) {
    for (pixel, target) in rgba.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let alpha = u32::from(pixel[3]);
        let scaled = |channel: u8| ((u32::from(channel) * alpha + 127) / 255) as u8;
        target[0] = scaled(pixel[2]);
        target[1] = scaled(pixel[1]);
        target[2] = scaled(pixel[0]);
        target[3] = pixel[3];
    }
}
