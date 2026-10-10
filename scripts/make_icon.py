"""Draw assets/parakeet.ico: the bars overlay's mirrored plumage bars in a night-blue squircle.

Each size is drawn on its own pixel grid, with fewer and thicker bars at the small sizes, so
nothing is a downscaled 256 px image. Pass --sheet to also save a contact sheet of every size.
"""

import argparse
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

from parakeet_dictate.styles.bars import PLUMAGE_STOPS

ROOT = Path(__file__).resolve().parent.parent
ICO_PATH = ROOT / "assets" / "parakeet.ico"
SUPERSAMPLE = 8
SQUIRCLE_POWER = 5.0
# The fluid overlay's NIGHT capsule, top to bottom.
NIGHT_TOP, NIGHT_BOTTOM = np.array([0x1A, 0x22, 0x34]) / 255, np.array([0x09, 0x0E, 0x18]) / 255
RIM = np.array([0xA9, 0xB8, 0xD6]) / 255
LIGHT_BG, TASKBAR_BG = (243, 243, 243), (32, 32, 32)


@dataclass(frozen=True)
class Spec:
    """One size's design, in that size's own pixels."""

    lengths: tuple[float, ...]  # tip to tip, per bar from left to right, as a fraction of the size
    width: float
    gap: float
    inset: float
    rim: bool
    glow: bool


SPECS = {
    16: Spec((0.44, 0.75, 0.44), width=2, gap=2, inset=0, rim=False, glow=False),
    20: Spec((0.4, 0.7, 0.4), width=3, gap=2, inset=0, rim=False, glow=False),
    24: Spec((0.34, 0.5, 0.67, 0.5, 0.34), width=2, gap=2, inset=0, rim=False, glow=False),
    32: Spec((0.32, 0.5, 0.69, 0.5, 0.32), width=3, gap=2, inset=1, rim=True, glow=False),
    40: Spec((0.3, 0.5, 0.67, 0.5, 0.3), width=4, gap=2, inset=1, rim=True, glow=False),
    48: Spec((0.21, 0.42, 0.33, 0.67, 0.33, 0.42, 0.21), width=3, gap=2, inset=1.5, rim=True, glow=True),
    64: Spec((0.19, 0.41, 0.31, 0.66, 0.31, 0.41, 0.19), width=4, gap=3, inset=2, rim=True, glow=True),
    128: Spec((0.14, 0.33, 0.25, 0.52, 0.66, 0.52, 0.25, 0.33, 0.14), width=7.5, gap=4, inset=4, rim=True, glow=True),
    256: Spec((0.14, 0.33, 0.25, 0.52, 0.66, 0.52, 0.25, 0.33, 0.14), width=15, gap=8, inset=8, rim=True, glow=True),
}


def plumage(t: float) -> np.ndarray:
    at = [p for p, _ in PLUMAGE_STOPS]
    rgb = np.array([c for _, c in PLUMAGE_STOPS], float) / 255
    return np.array([np.interp(t, at, rgb[:, i]) for i in range(3)])


def render(size: int, spec: Spec) -> Image.Image:
    n = size * SUPERSAMPLE
    coords = (np.arange(n) + 0.5) / SUPERSAMPLE
    x, y = coords[None, :], coords[:, None]
    mid = size / 2

    half = mid - spec.inset
    shape = (np.abs(x - mid) / half) ** SQUIRCLE_POWER + (np.abs(y - mid) / half) ** SQUIRCLE_POWER
    body = (shape <= 1).astype(float)
    depth = np.clip((y - spec.inset) / (2 * half), 0, 1)
    rgb = np.broadcast_to(NIGHT_TOP + (NIGHT_BOTTOM - NIGHT_TOP) * depth[..., None], (n, n, 3)).copy()

    if spec.rim:
        inner = half - max(1.0, size / 128)
        inner_shape = (np.abs(x - mid) / inner) ** SQUIRCLE_POWER + (np.abs(y - mid) / inner) ** SQUIRCLE_POWER
        ring = body * (inner_shape > 1)
        strength = ring * (0.06 + 0.24 * (1 - depth) ** 2)
        rgb += (RIM - rgb) * strength[..., None]

    count = len(spec.lengths)
    total = count * spec.width + (count - 1) * spec.gap
    # Whole-pixel bar edges keep the small sizes sharp; one spare pixel goes to the right.
    left = (size - total) // 2 if size <= 64 else (size - total) / 2
    bars = np.zeros((n, n, 3))
    cover = np.zeros((n, n))
    for i, fraction in enumerate(spec.lengths):
        k = abs(i - (count - 1) / 2) / ((count - 1) / 2)
        color = plumage(k * min(0.9, 0.3 + 0.1 * count))
        length = fraction * size
        if size <= 64:
            length = max(spec.width, 2 * round((length - size % 2) / 2) + size % 2)
        r = spec.width / 2
        cx = left + i * (spec.width + spec.gap) + r
        reach = np.maximum(np.abs(y - mid) - (length / 2 - r), 0)
        inside = ((x - cx) ** 2 + reach**2 <= r * r).astype(float)
        if size >= 48:
            along = np.clip(np.abs(y - mid) / (length / 2), 0, 1)
            shaded = color + (1 - color) * 0.22 * along[..., None] ** 2
        else:
            shaded = np.broadcast_to(color, (n, 1, 3))
        bars += shaded * inside[..., None]
        cover = np.maximum(cover, inside)

    if spec.glow:
        bloom = Image.fromarray((np.clip(bars, 0, 1) * 255).astype(np.uint8))
        bloom = bloom.filter(ImageFilter.GaussianBlur(size * 0.05 * SUPERSAMPLE))
        rgb += np.asarray(bloom, float) / 255 * 0.45
    rgb = rgb * (1 - cover[..., None]) + bars
    rgb = np.clip(rgb, 0, 1)

    premultiplied = np.concatenate([rgb * body[..., None], body[..., None]], axis=-1)
    pixels = premultiplied.reshape(size, SUPERSAMPLE, size, SUPERSAMPLE, 4).mean(axis=(1, 3))
    data = (pixels * 255 + 0.5).astype(np.uint8)
    return Image.fromarray(data, "RGBa").convert("RGBA")


def contact_sheet(images: dict[int, Image.Image]) -> Image.Image:
    """Every size at 1x, then the four smallest at 8x with hard pixels, on light and on a dark taskbar."""
    font = ImageFont.load_default(size=13)
    zoomed = [16, 20, 24, 32]
    pad = 24
    row1_h = max(images) + 26
    row2_h = 8 * max(zoomed) + 26
    panel_w = max(sum(images) + pad * (len(images) + 1), sum(8 * s for s in zoomed) + pad * (len(zoomed) + 1))
    panel_h = pad + row1_h + pad + row2_h + pad
    sheet = Image.new("RGB", (panel_w, 2 * panel_h))
    for p, background in enumerate((LIGHT_BG, TASKBAR_BG)):
        top = p * panel_h
        sheet.paste(background, (0, top, panel_w, top + panel_h))
        ink = (40, 40, 40) if p == 0 else (220, 220, 220)
        draw = ImageDraw.Draw(sheet)
        x = pad
        for s, image in images.items():
            sheet.paste(image, (x, top + pad + max(images) - s), image)
            draw.text((x, top + pad + max(images) + 6), str(s), fill=ink, font=font)
            x += s + pad
        x = pad
        y = top + pad + row1_h + pad
        for s in zoomed:
            big = images[s].resize((8 * s, 8 * s), Image.NEAREST)
            sheet.paste(big, (x, y + 8 * max(zoomed) - 8 * s), big)
            draw.text((x, y + 8 * max(zoomed) + 6), f"{s} at 8x", fill=ink, font=font)
            x += 8 * s + pad
    return sheet


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sheet", type=Path, help="also save a contact sheet PNG here")
    args = parser.parse_args()

    images = {size: render(size, spec) for size, spec in SPECS.items()}
    ICO_PATH.parent.mkdir(exist_ok=True)
    largest = max(images)
    images[largest].save(
        ICO_PATH,
        format="ICO",
        sizes=[(s, s) for s in images],
        append_images=[image for s, image in images.items() if s != largest],
    )
    with Image.open(ICO_PATH) as ico:
        for s, image in images.items():
            ico.size = (s, s)
            # Pillow resizes the largest image for any size it was not handed; this catches that.
            assert ico.convert("RGBA").tobytes() == image.tobytes(), f"{s} px frame is not the drawn one"
    print(f"wrote {ICO_PATH} with sizes {', '.join(map(str, images))}")

    if args.sheet:
        args.sheet.parent.mkdir(parents=True, exist_ok=True)
        contact_sheet(images).save(args.sheet)
        print(f"wrote {args.sheet}")


if __name__ == "__main__":
    main()
