"""Overlay renderer: a smoked-glass capsule whose mirrored bars carry every phase."""

import math
from dataclasses import dataclass, replace
from typing import Literal

import numpy as np
import numpy.typing as npt
from PIL import Image

from ..overlay_frame import BEAT_S, OverlayFrame, Phase

F32 = npt.NDArray[np.float32]
Glyph = Literal["latch", "check", "alert"]

N_BARS = 31
CENTER = (N_BARS - 1) / 2
K = np.abs(np.arange(N_BARS, dtype=np.float32) - CENTER)
K_NORM = K / CENTER
PITCH = 6.0
BAR_R = 1.5
GLOW_MARGIN = 8.5
L_MAX = 14.5
HOPS_PER_BAR = 2.0
CAPSULE_W, CAPSULE_H = 214.0, 46.0
LATCH_W = 14.0
CY = 58.0

GLASS_TOP = np.array([0.105, 0.115, 0.145], np.float32)
GLASS_BOTTOM = np.array([0.045, 0.052, 0.068], np.float32)
GLASS_ALPHA = 0.9


def _ramp(stops: list[tuple[float, tuple[int, int, int]]], t: F32) -> F32:
    at = [p for p, _ in stops]
    rgb = np.array([c for _, c in stops], np.float32) / 255
    return np.stack([np.interp(t, at, rgb[:, i]) for i in range(3)], axis=-1).astype(np.float32)


# Budgerigar plumage, yellow face to green body to blue tail: fresh sound at the centre, cooling as it ages outward.
# scripts/make_icon.py paints the app icon's bars with the same stops.
PLUMAGE_STOPS = [
    (0.0, (250, 255, 185)),
    (0.2, (205, 250, 110)),
    (0.47, (70, 228, 160)),
    (0.76, (38, 160, 205)),
    (1.0, (62, 84, 160)),
]
PLUMAGE = _ramp(PLUMAGE_STOPS, K_NORM)
IDLE_C = np.array([0.5, 0.72, 0.8], np.float32)
IRIS = [(0.0, (140, 92, 255)), (0.33, (225, 84, 255)), (0.66, (92, 140, 255)), (1.0, (140, 92, 255))]
SCAN = np.array([0.86, 0.93, 1.0], np.float32)
DONE_C = np.array([0.38, 0.94, 0.62], np.float32)
CANCEL_C = np.array([0.55, 0.57, 0.62], np.float32)
FAIL_C = np.array([1.0, 0.43, 0.43], np.float32)
LATCH_C = np.array([0.9, 0.92, 0.96], np.float32)


def _ease(t: float) -> float:
    t = min(1.0, max(0.0, t))
    return 1 - (1 - t) ** 3


@dataclass(frozen=True)
class Pose:
    """What the phase asks of the strip. Per-bar arrays run left to right; lengths are logical px."""

    halves: F32
    colors: F32
    alpha: F32
    aura: F32
    flare: float = 0.0
    width: float = 1.0
    pitch: float = 1.0
    shift: float = 0.0
    rim: F32 | None = None
    rim_strength: float = 0.0
    glyph: Glyph | None = None
    glyph_t: float = 0.0


def _recording(frame: OverlayFrame) -> Pose:
    levels = frame.levels[::-1]
    history = np.interp(K * HOPS_PER_BAR, np.arange(len(levels)), levels).astype(np.float32)
    voice = history**1.7 * (1 - 0.5 * K_NORM**2)
    # Silence ripples outward the same way speech does, only a pixel tall.
    breath = 0.5 + 0.5 * np.sin(2 * math.pi * 0.55 * frame.recording_elapsed - K * 0.55)
    lit = np.clip(history * 2.5, 0, 1)
    # Speech energy sits in bands 3 to 20 (about 150 Hz to 5 kHz); the lowest of those breathe at the centre.
    bands = np.interp(3 + K * (17 / CENTER), np.arange(len(frame.spectrum)), frame.spectrum)
    return Pose(
        halves=np.maximum(1.2 * breath, voice * L_MAX).astype(np.float32),
        colors=IDLE_C + (PLUMAGE - IDLE_C) * lit[:, None],
        alpha=((1 - 0.45 * K_NORM**2) * (0.55 + 0.45 * np.maximum(breath, lit))).astype(np.float32),
        aura=np.clip(bands * 1.8, 0, 1).astype(np.float32),
        flare=0.6,
        glyph="latch" if frame.hands_free else None,
    )


def _transcribing(frame: OverlayFrame) -> Pose:
    t = frame.phase_elapsed
    settle = _ease(t / 0.15)
    head = 1 - (t / 0.55) % 1.0
    bump = np.exp(-(((K_NORM - head) / 0.16) ** 2)).astype(np.float32)
    start = _recording(frame)
    working = 1.0 + 7.0 * bump
    scan = SCAN * (0.55 + 0.45 * bump[:, None])
    return Pose(
        halves=start.halves + (working - start.halves) * settle,
        colors=start.colors + (scan - start.colors) * settle,
        alpha=start.alpha + (1 - start.alpha) * settle,
        aura=start.aura * (1 - settle) + bump * 0.6 * settle,
        flare=start.flare * (1 - settle),
    )


def _polishing(frame: OverlayFrame) -> Pose:
    t = frame.phase_elapsed
    settle = _ease(t / 0.25)
    a = 0.5 + 0.5 * np.sin(2 * math.pi * 0.7 * t - K * 0.5)
    b = 0.5 + 0.5 * np.sin(2 * math.pi * 0.27 * t + K * 0.23 + 1.1)
    working = 1.5 + 9.0 * a * (0.35 + 0.65 * b) * (1 - 0.45 * K_NORM**2)
    colors = _ramp(IRIS, (K / (N_BARS * 1.2) - t * 0.35) % 1.0)
    return Pose(
        halves=(1.0 + (working - 1.0) * settle).astype(np.float32),
        colors=SCAN + (colors - SCAN) * settle,
        alpha=np.ones(N_BARS, np.float32),
        aura=(0.25 + 0.5 * a * b).astype(np.float32) * settle,
    )


def _ending(frame: OverlayFrame, color: F32, glyph: Glyph | None) -> Pose:
    t = frame.phase_elapsed
    fall = _ease(t / 0.14)
    clear = np.clip((K - 3.0) / 2.0, 0, 1) if glyph else np.ones(N_BARS)
    alpha = (clear + (1 - clear) * (1 - fall)) * (1 - 0.4 * fall)
    return Pose(
        halves=np.full(N_BARS, 1.0 - fall, np.float32),
        colors=np.broadcast_to(color, (N_BARS, 3)),
        alpha=alpha.astype(np.float32),
        aura=np.zeros(N_BARS, np.float32),
        rim=color,
        rim_strength=math.exp(-t * 6.0),
        glyph=glyph,
        glyph_t=t,
    )


def _done(frame: OverlayFrame) -> Pose:
    pose = _ending(frame, DONE_C, "check")
    wave = (np.exp(-(((K_NORM - frame.phase_elapsed / 0.32) / 0.14) ** 2)) * (1 - K_NORM) ** 1.5).astype(np.float32)
    return replace(pose, halves=pose.halves + 3.0 * wave, aura=wave * 0.8)


def _cancelled(frame: OverlayFrame) -> Pose:
    fold = _ease(frame.phase_elapsed / BEAT_S)
    pose = _ending(frame, CANCEL_C, None)
    return replace(pose, pitch=1 - 0.55 * fold, width=1 - 0.4 * fold, rim_strength=0.0)


def _failed(frame: OverlayFrame) -> Pose:
    t = frame.phase_elapsed
    shake = 4.0 * math.exp(-t * 7.0) * math.sin(2 * math.pi * 7.5 * t)
    return replace(_ending(frame, FAIL_C, "alert"), shift=shake)


POSES = {
    Phase.RECORDING: _recording,
    Phase.TRANSCRIBING: _transcribing,
    Phase.POLISHING: _polishing,
    Phase.DONE: _done,
    Phase.CANCELLED: _cancelled,
    Phase.FAILED: _failed,
}


def _over(c: F32, a: F32, rgb: F32, alpha: F32) -> None:
    """Composite straight `rgb` at `alpha` over premultiplied planar `c` (3, rows, cols) and `a`."""
    keep = 1 - alpha
    c *= keep
    c += rgb * alpha
    a *= keep
    a += alpha


def _segment(px: F32, py: F32, ax: float, ay: float, bx: float, by: float) -> F32:
    dx, dy = bx - ax, by - ay
    h = np.clip(((px - ax) * dx + (py - ay) * dy) / max(dx * dx + dy * dy, 1e-6), 0, 1)
    return np.hypot(px - ax - dx * h, py - ay - dy * h)


def _glyph(kind: Glyph, t: float, px: F32, py: F32, s: float) -> tuple[F32, F32]:
    """Signed distance in device pixels, and colour, of a glyph centred on (0, 0)."""
    if kind == "latch":
        qx, qy = np.abs(px) - 3.3 * s, np.abs(py - 2.0 * s) - 2.3 * s
        body = np.hypot(np.maximum(qx, 0), np.maximum(qy, 0)) + np.minimum(np.maximum(qx, qy), 0) - 1.0 * s
        arc = np.abs(np.hypot(px, py + 2.0 * s) - 2.5 * s)
        leg = _segment(np.abs(px), py, 2.5 * s, -2.0 * s, 2.5 * s, -0.5 * s)
        shackle = np.where(py < -2.0 * s, arc, leg) - 0.75 * s
        return np.minimum(body, shackle), LATCH_C
    if kind == "check":
        drawn = _ease((t - 0.04) / 0.22)
        ax, ay, bx, by, cx, cy = -7.0 * s, 0.4 * s, -2.3 * s, 5.0 * s, 7.6 * s, -5.8 * s
        first = 0.36
        u = min(1.0, drawn / first)
        d = _segment(px, py, ax, ay, ax + (bx - ax) * u, ay + (by - ay) * u)
        if drawn > first:
            u = (drawn - first) / (1 - first)
            d = np.minimum(d, _segment(px, py, bx, by, bx + (cx - bx) * u, by + (cy - by) * u))
        return d - 1.6 * s, DONE_C
    stem = _segment(px, py, 0, -7.0 * s, 0, 1.6 * s) - 1.6 * s
    dot = np.hypot(px, py - 6.2 * s) - 1.75 * s
    return np.minimum(stem, dot), FAIL_C


def _shell(qx2: F32, dy: F32, half_h: float, s: float, rim_rgb: F32, rim_strength: float) -> tuple[F32, F32, F32]:
    """Premultiplied colour, alpha and signed distance of the shadowed glass capsule at these columns."""
    d = np.sqrt(qx2[None, :] + (dy**2)[:, None]) - half_h
    shadow_d = np.sqrt(qx2[None, :] + ((dy - 4 * s) ** 2)[:, None]) - half_h
    shadow = np.maximum(
        0.34 * np.clip(1 - (shadow_d + 2 * s) / (13 * s), 0, 1) ** 2,
        0.22 * np.clip(1 - (shadow_d - 2 * s) / (3 * s), 0, 1),
    )
    cover = np.clip(0.5 - d, 0, 1) * GLASS_ALPHA
    depth = np.clip((dy + half_h) / (2 * half_h), 0, 1)
    glass = GLASS_TOP[:, None] + (GLASS_BOTTOM - GLASS_TOP)[:, None] * depth[None, :]
    c = glass[:, :, None] * cover[None]
    a = cover + shadow * (1 - cover)
    # A light hairline just inside the edge holds the shape on dark backgrounds, a dark one just
    # outside holds it on white; the top of the light one doubles as the glass highlight.
    top = np.clip(-dy / half_h, 0, 1)[:, None] ** 1.5
    inner = np.clip(1 - np.abs(d + s) / (0.75 * s), 0, 1) * (0.1 + 0.22 * top + 0.55 * rim_strength)
    outer = np.clip(1 - np.abs(d - 0.5 * s) / (0.75 * s), 0, 1) * 0.35
    _over(c, a, rim_rgb[:, None, None] * (inner / np.maximum(inner + outer, 1e-6)), np.maximum(inner, outer))
    return c, a, d


def _bars(pose: Pose, c: F32, a: F32, d: F32, x: F32, y: F32, cx: float, cy: float, pitch: float, s: float) -> None:
    """Draw the mirrored bars, their bloom and the spectral aura into the premultiplied `c` and `a`."""
    centers = cx + (np.arange(N_BARS, dtype=np.float32) - CENTER) * pitch
    idx = np.clip(np.rint((x - centers[0]) / pitch), 0, N_BARS - 1).astype(np.intp)
    dx = x - centers[idx]
    dy = np.abs(y - cy)
    interior = np.clip(-1.5 * s - d, 0, 1)

    reach = np.maximum(dy[:, None] - (pose.halves[idx] * s)[None, :], 0)
    bar_d = np.sqrt((dx**2)[None, :] + reach**2) - BAR_R * s
    bar_a = np.clip(0.5 - bar_d, 0, 1) * pose.alpha[idx][None, :] * interior

    # Brighter toward the tips, and loud tips flare toward white.
    along = np.clip(dy / ((L_MAX + BAR_R) * s), 0, 1)
    white = pose.flare * along**2.5
    gain = (0.7 + 0.3 * along) * (1 - white)

    keep = 1 - bar_a
    bloom = np.exp(-np.maximum(bar_d, 0) / (1.8 * s)) * keep * interior * 0.3
    # The aura reaches zero at the box edge so it never shows a seam.
    margin = GLOW_MARGIN * s
    xp = np.r_[centers[0] - margin, centers, centers[-1] + margin]
    amp = np.interp(x, xp, np.r_[0, pose.aura, 0]).astype(np.float32)
    aura = np.exp(-((dy[:, None] / ((5 + 12 * amp) * s)[None, :]) ** 2)) * (amp * 0.32)[None, :] * interior

    lit = bar_a + bloom
    c *= keep
    c += pose.colors[idx].T[:, None, :] * (gain[:, None] * lit + aura)
    c += white[:, None] * lit
    a *= keep
    a += bar_a


def render(frame: OverlayFrame) -> Image.Image:
    w, h = frame.size
    if frame.presence <= 0:
        return Image.new("RGBA", (w, h), (0, 0, 0, 0))
    s = frame.scale
    pose = POSES[frame.phase](frame)

    # A dot fades in, then stretches into the capsule; leaving plays it backwards.
    p = frame.presence
    full_w = (CAPSULE_W + (2 * LATCH_W if frame.hands_free else 0)) * pose.width
    dot = CAPSULE_H * (0.5 + 0.5 * _ease(p / 0.6))
    half_h = dot * s / 2
    half_w = (dot + (full_w - dot) * _ease((p - 0.15) / 0.85)) * s / 2
    cx = w / 2 + pose.shift * s
    cy = CY * s

    x0, x1 = max(0, int(cx - half_w - 12 * s)), min(w, math.ceil(cx + half_w + 12 * s))
    y0, y1 = max(0, int(cy - half_h - 8 * s)), min(h, math.ceil(cy + half_h + 16 * s))
    x = np.arange(x0, x1, dtype=np.float32) + 0.5
    y = np.arange(y0, y1, dtype=np.float32) + 0.5

    # Between the end caps every column is identical, so the shell is computed for the caps and
    # one middle column, and that column is repeated across the middle.
    flat = half_w - half_h
    qx2 = np.maximum(np.abs(x - cx) - flat, 0) ** 2
    m0 = int(np.searchsorted(x, cx - flat, "left"))
    flat_n = max(0, int(np.searchsorted(x, cx + flat, "right")) - m0)
    distinct = np.concatenate((qx2[: m0 + 1], qx2[m0 + flat_n :])) if flat_n else qx2
    counts = np.ones(len(distinct), np.intp)
    if flat_n:
        counts[m0] = flat_n
    rim_rgb = np.ones(3, np.float32) if pose.rim is None else pose.rim
    c, a, d = (np.repeat(v, counts, axis=-1) for v in _shell(distinct, y - cy, half_h, s, rim_rgb, pose.rim_strength))

    def box(bx: float, by: float, rx: float, ry: float) -> tuple[slice, slice]:
        return slice(max(0, int(by - ry) - y0), max(0, math.ceil(by + ry) - y0)), slice(
            max(0, int(bx - rx) - x0), max(0, math.ceil(bx + rx) - x0)
        )

    pitch = PITCH * pose.pitch * s
    rows, cols = box(cx, cy, CENTER * pitch + GLOW_MARGIN * s, (L_MAX + BAR_R + 6.5) * s)
    _bars(pose, c[:, rows, cols], a[rows, cols], d[rows, cols], x[cols], y[rows], cx, cy, pitch, s)

    if pose.glyph:
        gx = cx - half_w + 17.5 * s if pose.glyph == "latch" else cx
        rows, cols = box(gx, cy, 14 * s, 14 * s)
        gd, g_rgb = _glyph(pose.glyph, pose.glyph_t, (x[cols] - gx)[None, :], (y[rows] - cy)[:, None], s)
        inside = np.clip(-1.5 * s - d[rows, cols], 0, 1)
        g_a = np.clip(0.5 - gd, 0, 1) * inside
        _over(c[:, rows, cols], a[rows, cols], g_rgb[:, None, None], g_a)
        if pose.glyph != "latch":
            glow = np.exp(-np.maximum(gd, 0) / (2.5 * s)) * (1 - g_a) * inside * 0.35
            c[:, rows, cols] += g_rgb[:, None, None] * glow

    # Additive light can push premultiplied colour past alpha; clamp it, then let Pillow unpremultiply.
    np.minimum(c, a, out=c)
    gain = 255 * min(1.0, p / 0.3)
    bands = np.zeros((4, h, w), np.uint8)
    bands[:3, y0:y1, x0:x1] = c * gain + 0.5
    bands[3, y0:y1, x0:x1] = a * gain + 0.5
    return Image.merge("RGBa", [Image.fromarray(band) for band in bands]).convert("RGBA")
