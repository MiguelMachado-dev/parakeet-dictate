"""Plumage: voice ribbons in budgerigar colours, held in a night-blue capsule."""

import math
from collections.abc import Callable
from dataclasses import dataclass
from itertools import pairwise

import numpy as np
import numpy.typing as npt
from PIL import Image

from ..overlay_frame import BEAT_S, OverlayFrame, Phase

F32 = npt.NDArray[np.float32]

# Logical pixels, multiplied by frame.scale.
HALF_LEN = 134.0
RADIUS = 26.0
LIFT = 3.0
SHADOW_DROP = 4.0
SHADOW_REACH = 15.0
SHADOW_ALPHA = 0.45
HALO_REACH = 13.0
HALO_ALPHA = 0.55
CREST_ROOM = 5.0
CAP_REACH = 0.55
LOCK_ROOM = 16.0
# How long the hands-free lock lingers into each phase that can directly follow a latched recording.
LOCK_FADE_S = {Phase.TRANSCRIBING: 0.08, Phase.CANCELLED: 0.16}
GLOW_REACH = 7.0

MEMBRANE, EDGE, GLOW = 0.3, 1.0, 0.3

# History hops a ripple takes to travel from the middle of the capsule to its ends.
RIPPLE_HOPS = 34.0
RIPPLE_CORE = 0.14
SMOOTH = np.array([1, 2, 3, 2, 1], np.float32) / 9


def _rgb(hex_code: str) -> F32:
    return np.array([int(hex_code[i : i + 2], 16) / 255 for i in (1, 3, 5)], np.float32)


def _body(top: str, bottom: str) -> F32:
    return np.stack([_rgb(top), _rgb(bottom)])


NIGHT = _body("#1a2234", "#090e18")
INDIGO = _body("#251a4a", "#0d0920")
MOSS = _body("#173a26", "#08170f")
WINE = _body("#3a1820", "#170a0d")
GRAPHITE = _body("#20232a", "#0d0f13")
RIM = _rgb("#a9b8d6")
WHITE = _rgb("#ffffff")
COBALT, SKY, GREEN, LEMON = _rgb("#3a6bff"), _rgb("#1fc8f0"), _rgb("#79ec4f"), _rgb("#ffd84a")
VIOLET, ORCHID = _rgb("#8f63ff"), _rgb("#e46cff")
CORAL, ASH = _rgb("#ff6a5c"), _rgb("#9aa3b2")
LOCK_INK, SPARKLE_INK, CHECK_INK = _rgb("#dfe6f5"), _rgb("#f1e9ff"), _rgb("#b6ff8f")
PLUMAGE = np.stack([COBALT, SKY, GREEN, LEMON])
THINKING = np.stack([VIOLET, SKY, ORCHID, COBALT])

# Per ribbon: spatial cycles across the span, phase speed (rad/s), phase offset, twist cycles.
CYCLES = np.array([1.25, 1.7, 2.2, 2.85], np.float32)[:, None]
SPEED = np.array([2.1, -2.7, 3.3, -4.1], np.float32)[:, None]
OFFSET = np.array([0.0, 2.1, 4.0, 5.5], np.float32)[:, None]
TWIST = np.array([0.9, 1.3, 1.1, 1.6], np.float32)[:, None]
QUARTERS = np.arange(4, dtype=np.float32)[:, None] * (np.pi / 2)


@dataclass(frozen=True)
class Look:
    """Four ribbons sampled at each column of the capsule, and the light and body around them."""

    center: F32  # (4, X) crest offset from the midline, px
    half: F32  # (4, X) half-width of the ribbon membrane, px
    gain: F32  # (4, X) brightness
    color: F32  # (4, 3)
    aura: F32  # (3, X) soft light washed through the capsule behind the ribbons
    body: F32  # (2, 3) top and bottom of the capsule

    def mix(self, other: "Look", m: float) -> "Look":
        if m <= 0:
            return self
        if m >= 1:
            return other
        return Look(*(a + (b - a) * m for a, b in zip(vars(self).values(), vars(other).values())))


@dataclass(frozen=True)
class Stage:
    frame: OverlayFrame
    u: F32  # (X,) column position along the ribbon span, -1..1
    taper: F32  # (X,) 1 mid-span, 0 at the span ends
    reach: float  # px a crest may travel from the midline
    s: float

    @property
    def t(self) -> float:
        return self.frame.phase_elapsed


def _smoothstep(e0: float, e1: float, x: float) -> float:
    x = min(max((x - e0) / (e1 - e0), 0.0), 1.0)
    return x * x * (3.0 - 2.0 * x)


def _ripple(levels: F32, u: F32) -> F32:
    """The level history laid out from the middle (now) to the span ends (RIPPLE_HOPS ago)."""
    padded = np.concatenate((levels[:1], levels[:1], levels, levels[-1:], levels[-1:]))
    smooth = np.convolve(padded, SMOOTH, mode="valid")
    # Hyperbolic rather than |u|, so a fresh onset swells as a rounded crest instead of a spike.
    away = (np.sqrt(u * u + RIPPLE_CORE**2) - RIPPLE_CORE) / (np.sqrt(1 + RIPPLE_CORE**2) - RIPPLE_CORE)
    age = len(smooth) - 1 - away * RIPPLE_HOPS
    return np.interp(age, np.arange(len(smooth)), smooth).astype(np.float32)


def _voice(st: Stage) -> Look:
    f = st.frame
    energy = f.spectrum.reshape(4, -1).mean(axis=1)
    share = (energy / max(float(energy.max()), 0.05)).astype(np.float32)
    tint = (energy @ PLUMAGE + 0.04 * SKY) / (float(energy.sum()) + 0.04)
    drive = _ripple(f.levels, st.u)[None, :] * (0.55 + 0.45 * share[:, None])
    t = f.recording_elapsed
    u = st.u[None, :]
    breathe = 0.85 + 0.15 * np.sin(2 * np.pi * t / 2.6 + OFFSET)
    phase = np.pi * CYCLES * u + SPEED * t + OFFSET
    wave = 0.8 * np.sin(phase) + 0.2 * np.sin(0.55 * phase - 1.7 * t)
    amp = st.reach * st.taper * np.minimum(0.3 * breathe + 1.1 * drive, 1.0)
    twist = np.abs(np.cos(np.pi * TWIST * u - 1.3 * t + OFFSET))
    half = st.s * (0.5 + (0.7 + 5.0 * drive) * twist) * st.taper**0.7
    gain = (0.6 + 1.1 * drive) * breathe * st.taper**0.6
    aura = tint[:, None] * ((0.05 + 0.4 * float(f.levels[-3:].mean())) * st.taper**1.5)
    return Look(amp * wave, half, gain, PLUMAGE, aura, NIGHT)


def _rope(st: Stage) -> Look:
    u = st.u[None, :]
    phase = np.pi * 3.0 * u - 16.0 * st.t + QUARTERS
    sweep = -1.15 + 2.55 * ((st.t / 0.5) % 1.0)
    shine = np.exp(-(((st.u - sweep) / 0.16) ** 2))
    center = 0.16 * st.reach * st.taper**1.2 * np.sin(phase)
    half = st.s * (0.3 + 0.6 * np.abs(np.cos(phase))) * st.taper**0.7
    gain = np.broadcast_to((0.6 + 2.2 * shine) * st.taper**0.6, half.shape)
    aura = (SKY + (WHITE - SKY) * 0.5)[:, None] * ((0.06 + 0.3 * shine) * st.taper)
    return Look(center, half, gain, PLUMAGE + (WHITE - PLUMAGE) * 0.5, aura, NIGHT)


def _transcribing(st: Stage) -> Look:
    return _voice(st).mix(_rope(st), _smoothstep(0.0, 0.08, st.t))


def _polishing(st: Stage) -> Look:
    u = st.u[None, :]
    t = st.t
    sign = np.array([1, -1, 1, -1], np.float32)[:, None]
    lag = np.array([0, 0, 1, 1], np.float32)[:, None] * (np.pi / 2)
    size = np.array([1, 1, 0.55, 0.55], np.float32)[:, None]
    breath = math.sin(2 * np.pi * t / 1.9)
    phase = np.pi * 1.6 * u - 2.4 * t + lag
    center = sign * size * (0.5 + 0.16 * breath) * st.reach * st.taper**1.3 * np.sin(phase)
    half = st.s * (0.5 + 3.2 * np.abs(np.cos(phase))) * st.taper**0.7
    shimmer = 0.5 + 0.5 * np.cos(1.2 * np.pi * u - 1.7 * t + QUARTERS)
    spark_at = -1.3 + 2.6 * ((t / 1.7 + QUARTERS * 0.17) % 1.0)
    spark = np.exp(-(((u - spark_at) / 0.05) ** 2))
    gain = (shimmer + 1.6 * spark) * st.taper**0.6
    aura = VIOLET[:, None] * ((0.14 + 0.06 * breath) * st.taper**1.5)
    helix = Look(center, half, gain, THINKING, aura, INDIGO)
    return _rope(st).mix(helix, _smoothstep(0.0, 0.3, t))


def _line(st: Stage, color: F32, body: F32, span: float, flash: float, droop: float = 0.0) -> Look:
    kept = np.clip(1.0 - np.abs(st.u) / max(span, 1e-3), 0.0, 1.0)
    taper = np.minimum(st.taper, kept * 3.0)
    zero = np.zeros((4, len(st.u)), np.float32)
    center = zero + droop * st.reach * (1.0 - st.u * st.u) * st.taper
    half = zero + st.s * 0.9 * taper**0.7
    aura = color[:, None] * (0.12 * flash * st.taper)
    return Look(center, half, zero + flash * taper**0.6, np.broadcast_to(color, (4, 3)), aura, body)


def _done(st: Stage) -> Look:
    t = min(st.t, BEAT_S)
    flash = 0.9 + 1.4 * math.exp(-(((t - 0.08) / 0.07) ** 2))
    line = _line(st, GREEN + (WHITE - GREEN) * 0.25, MOSS, 1.0 - _smoothstep(0.04, 0.17, t), flash)
    return _rope(st).mix(line, _smoothstep(0.0, 0.08, t))


def _cancelled(st: Stage) -> Look:
    t = min(st.t, BEAT_S)
    line = _line(st, ASH, GRAPHITE, 1.0 - _smoothstep(0.12, 0.42, t), 0.7)
    return _voice(st).mix(line, _smoothstep(0.0, 0.16, t))


def _failed(st: Stage) -> Look:
    t = min(st.t, BEAT_S)
    line = _line(st, CORAL, WINE, 1.0, 1.1, droop=0.3 * _smoothstep(0.05, 0.3, t))
    return _rope(st).mix(line, _smoothstep(0.0, 0.1, t))


LOOKS: dict[Phase, Callable[[Stage], Look]] = {
    Phase.RECORDING: _voice,
    Phase.TRANSCRIBING: _transcribing,
    Phase.POLISHING: _polishing,
    Phase.DONE: _done,
    Phase.CANCELLED: _cancelled,
    Phase.FAILED: _failed,
}


def _ribbons(look: Look, ys: F32, s: float) -> F32:
    """Tone-mapped additive light (3, rows, X) of the four ribbons over rows ys (offsets from the midline)."""
    c = look.center
    slope = np.empty_like(c)
    slope[:, 1:-1] = (c[:, 2:] - c[:, :-2]) * 0.5
    slope[:, 0], slope[:, -1] = slope[:, 1], slope[:, -2]
    # In place throughout: at scale 1.5 each temporary is ~0.6 MB and fresh ones cost page faults.
    edge = ys[None, :, None] - c[:, None, :]
    np.abs(edge, out=edge)
    edge *= (1.0 / np.sqrt(1.0 + slope * slope))[:, None, :]
    edge -= look.half[:, None, :]
    acc = np.clip(0.5 - edge, 0.0, 1.0)
    acc *= MEMBRANE
    tmp = np.abs(edge)
    np.subtract(1.0, tmp, out=tmp)
    np.maximum(tmp, 0.0, out=tmp)
    tmp *= EDGE
    acc += tmp
    np.multiply(edge, -1.0 / (s * GLOW_REACH), out=tmp)
    tmp += 1.0
    np.clip(tmp, 0.0, 1.0, out=tmp)
    tmp *= tmp
    tmp *= tmp
    tmp *= GLOW
    acc += tmp
    acc *= look.gain[:, None, :]
    light = np.tensordot(look.color.T, acc, axes=1)
    np.negative(light, out=light)
    np.exp(light, out=light)
    np.subtract(1.0, light, out=light)
    return light


def _segment(px: F32, py: F32, a: tuple[float, float], b: tuple[float, float]) -> F32:
    bax, bay = b[0] - a[0], b[1] - a[1]
    pax, pay = px - a[0], py - a[1]
    h = np.clip((pax * bax + pay * bay) / max(bax * bax + bay * bay, 1e-9), 0.0, 1.0)
    return np.hypot(pax - bax * h, pay - bay * h)


def _lock_sdf(px: F32, py: F32) -> F32:
    qx, qy = np.abs(px) - 3.3, np.abs(py - 2.6) - 2.3
    body = np.hypot(np.maximum(qx, 0), np.maximum(qy, 0)) + np.minimum(np.maximum(qx, qy), 0) - 1.2
    arc = np.abs(np.hypot(px, py + 0.6) - 2.8) - 0.85
    legs = np.maximum(np.abs(np.abs(px) - 2.8) - 0.85, py - 2.0)
    return np.minimum(body, np.where(py < -0.6, arc, legs))


def _sparkle_sdf(px: F32, py: F32, size: float, turn: float) -> F32:
    c, s = math.cos(turn), math.sin(turn)
    x, y = np.abs(c * px - s * py), np.abs(s * px + c * py)
    thin = size * 0.3
    norm = size * thin / math.hypot(size, thin)
    return np.minimum((x / size + y / thin - 1) * norm, (x / thin + y / size - 1) * norm)


def _check_sdf(px: F32, py: F32, progress: float) -> F32:
    pts = ((-7.6, 0.4), (-2.4, 5.6), (8.0, -5.6))
    lengths = [math.hypot(b[0] - a[0], b[1] - a[1]) for a, b in pairwise(pts)]
    left = progress * sum(lengths)
    d = np.full(np.broadcast(px, py).shape, np.inf, np.float32)
    for (a, b), length in zip(pairwise(pts), lengths):
        k = min(left / length, 1.0)
        if k <= 0:
            break
        d = np.minimum(d, _segment(px, py, a, (a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k)))
        left -= length
    return d - 1.9


def _latched(frame: OverlayFrame) -> float:
    """How present the hands-free lock is: fully while recording, fading out of the phases that follow it."""
    if not frame.hands_free:
        return 0.0
    if frame.phase is Phase.RECORDING:
        return 1.0
    fade = LOCK_FADE_S.get(frame.phase)
    return 0.0 if fade is None else 1.0 - _smoothstep(0.0, fade, frame.phase_elapsed)


Glyph = tuple[Callable[[F32, F32], F32], F32, str, float]


def _glyph(frame: OverlayFrame) -> Glyph | None:
    """The phase's glyph: an SDF in logical px, its colour, where it sits, and its opacity."""
    t = frame.phase_elapsed
    if (latch := _latched(frame)) > 0:
        return _lock_sdf, LOCK_INK, "left", latch
    if frame.phase is Phase.POLISHING:
        size = 6.0 * (1.0 + 0.2 * math.sin(2 * np.pi * t / 0.95)) * max(_smoothstep(0.05, 0.35, t), 0.05)
        turn = 0.35 * math.sin(2 * np.pi * t / 3.1)
        return (lambda x, y: _sparkle_sdf(x, y, size, turn)), SPARKLE_INK, "left", 1.0
    if frame.phase is Phase.DONE:
        progress = _smoothstep(0.14, 0.32, min(t, BEAT_S))
        if progress > 0:
            return (lambda x, y: _check_sdf(x, y, progress)), CHECK_INK, "center", 1.0
    return None


def render(frame: OverlayFrame) -> Image.Image:
    w, h = frame.size
    if frame.presence <= 0:
        return Image.new("RGBA", (w, h), (0, 0, 0, 0))
    s, p = frame.scale, frame.presence
    radius = s * RADIUS * (0.4 + 0.6 * _smoothstep(0.0, 0.5, p))
    half_len = s * HALF_LEN * _smoothstep(0.15, 1.0, p) ** 0.8
    shake = 0.0
    if frame.phase is Phase.FAILED:
        t = min(frame.phase_elapsed, BEAT_S)
        shake = s * 5.0 * math.sin(2 * np.pi * 11 * t) * math.exp(-t / 0.12)
    cx, cy = w / 2 + shake, h / 2 - s * LIFT

    reach = s * max(SHADOW_REACH, HALO_REACH)
    ya, yb = max(0, int(cy - radius - reach) - 1), min(h, int(cy + radius + s * SHADOW_DROP + reach) + 2)
    xa, xb = max(0, int(cx - half_len - radius - reach) - 1), min(w, int(cx + half_len + radius + reach) + 2)
    xs = np.arange(xa, xb, dtype=np.float32) + 0.5
    dy = np.arange(ya, yb, dtype=np.float32) + 0.5 - cy
    dx = np.maximum(np.abs(xs - cx) - half_len, 0.0)
    dx2 = dx * dx
    d = np.sqrt(dx2[None, :] + (dy * dy)[:, None]) - radius
    dys = dy - s * SHADOW_DROP
    d_shadow = np.sqrt(dx2[None, :] + (dys * dys)[:, None]) - radius
    cover = np.clip(0.5 - d, 0.0, 1.0)
    fall = np.clip(1.0 - d_shadow * (1.0 / (s * SHADOW_REACH)), 0.0, 1.0)
    under = SHADOW_ALPHA * fall * fall

    ra, rb = max(0, int(cy - radius) - 1 - ya), min(yb - ya, int(cy + radius) + 2 - ya)
    ca, cb = max(0, int(cx - half_len - radius) - 1 - xa), min(xb - xa, int(cx + half_len + radius) + 2 - xa)
    box_dy, box_xs = dy[ra:rb], xs[ca:cb]

    left = cx - half_len - CAP_REACH * radius + s * LOCK_ROOM * _latched(frame)
    right = cx + half_len + CAP_REACH * radius
    mid, span = (left + right) / 2, max((right - left) / 2, 1.0)
    u = (box_xs - mid) / span
    taper = np.clip(1.0 - u * u, 0.0, 1.0)
    look = LOOKS[frame.phase](Stage(frame, u, taper, max(radius - s * CREST_ROOM, 1.0), s))
    content = _smoothstep(0.05, 0.5, p)

    halo_light = look.aura.mean(axis=1) * content
    halo_peak = float(halo_light.max())
    halo = None
    if halo_peak > 1e-3:
        ring = np.clip(1.0 - d * (1.0 / (s * HALO_REACH)), 0.0, 1.0)
        halo = (HALO_ALPHA * min(halo_peak * 3.0, 1.0)) * ring * ring * ring
        under = halo + under * (1.0 - halo)
    alpha = cover + under * (1.0 - cover)

    v = np.clip((box_dy + radius) / (2 * radius), 0.0, 1.0)
    top, bottom = look.body[:, :, None, None]
    grad = top + (bottom - top) * v[:, None]
    rim = np.clip(1.0 - np.abs(d[ra:rb, ca:cb] + 1.0), 0.0, 1.0) * (0.14 + 0.4 * (1.0 - v[:, None]) ** 2)
    rgb = grad + (RIM[:, None, None] - grad) * rim
    q = box_dy * (1.6 / radius)
    aura = 1.0 - np.exp(-(look.aura * content)[:, None, :] * (1.0 / (1.0 + q * q))[None, :, None])
    rgb += aura * (1.0 - rgb)

    extent = float(np.max(np.abs(look.center) + look.half)) + s * (GLOW_REACH + 1.0)
    rows = np.nonzero(np.abs(box_dy) <= extent)[0]
    if len(rows) and content > 0:
        band = rgb[:, rows[0] : rows[-1] + 1]
        band += _ribbons(look, box_dy[rows[0] : rows[-1] + 1], s) * content * (1.0 - band)

    glyph = _glyph(frame)
    if glyph is not None:
        sdf, ink, anchor, strength = glyph
        gx = cx - half_len if anchor == "left" else cx
        near = s * 12.0
        gr = np.nonzero(np.abs(box_dy) < near)[0]
        gc = np.nonzero(np.abs(box_xs - gx) < near)[0]
        if len(gr) and len(gc):
            r0, r1, c0, c1 = gr[0], gr[-1] + 1, gc[0], gc[-1] + 1
            g = 0.5 - sdf(((box_xs[c0:c1] - gx) / s)[None, :], (box_dy[r0:r1] / s)[:, None]) * s
            g = np.clip(g, 0.0, 1.0) * (content * strength)
            patch = rgb[:, r0:r1, c0:c1]
            patch += (ink[:, None, None] - patch) * g

    # Straight alpha: un-premultiply the body and the halo spill by the combined coverage.
    inv = 255.0 / np.maximum(alpha, 1e-6)
    if halo is None:
        planes = np.zeros((3, *alpha.shape), np.float32)
    else:
        planes = (halo * (1.0 - cover) * inv)[None] * (halo_light / halo_peak)[:, None, None]
    np.clip(rgb, 0.0, 1.0, out=rgb)
    planes[:, ra:rb, ca:cb] += rgb * (cover * inv)[None, ra:rb, ca:cb]
    planes += 0.5
    rgb8 = planes.astype(np.uint8)
    alpha8 = (alpha * (255.0 * _smoothstep(0.0, 0.3, p)) + 0.5).astype(np.uint8)
    image = Image.merge("RGBA", [Image.fromarray(plane) for plane in (*rgb8, alpha8)])
    if image.size == (w, h):
        return image
    canvas = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    canvas.paste(image, (xa, ya))
    return canvas
