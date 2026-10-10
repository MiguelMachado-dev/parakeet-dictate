"""Placeholder overlay renderer: a flat pill with level bars. A designed renderer replaces it."""

from PIL import Image, ImageDraw

from .overlay_frame import OverlayFrame, Phase

COLORS = {
    Phase.RECORDING: (229, 57, 53),
    Phase.TRANSCRIBING: (255, 179, 0),
    Phase.POLISHING: (142, 36, 170),
    Phase.DONE: (67, 160, 71),
    Phase.CANCELLED: (158, 158, 158),
    Phase.FAILED: (229, 57, 53),
}


def render(frame: OverlayFrame) -> Image.Image:
    w, h = frame.size
    image = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    if frame.presence <= 0:
        return image
    s = frame.scale
    alpha = round(255 * frame.presence)
    draw = ImageDraw.Draw(image)
    pill = (w * 0.2, h * 0.3, w * 0.8, h * 0.7)
    draw.rounded_rectangle(pill, radius=h * 0.2, fill=(20, 20, 24, round(alpha * 0.9)))
    r, g, b = COLORS[frame.phase]
    bars = frame.levels[-24:]
    x0, x1 = pill[0] + 16 * s, pill[2] - 16 * s
    step = (x1 - x0) / len(bars)
    mid = h / 2
    for i, level in enumerate(bars):
        half = max(1.5 * s, float(level) * h * 0.17)
        x = x0 + i * step + step / 2
        draw.rounded_rectangle((x - 1.5 * s, mid - half, x + 1.5 * s, mid + half), radius=1.5 * s, fill=(r, g, b, alpha))
    return image
