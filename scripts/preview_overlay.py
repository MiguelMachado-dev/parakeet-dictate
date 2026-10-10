"""Render an overlay renderer through a scripted dictation timeline driven by the fixture audio.

Writes video.mp4 (needs ffmpeg on PATH), contact_dark.png, contact_light.png and timing.txt.
"""

import argparse
import importlib
import shutil
import subprocess
import sys
import time
import wave
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

from parakeet_dictate.meter import LevelMeter
from parakeet_dictate.overlay_frame import BEAT_S, FPS, OverlayFrame, Phase

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "tests" / "fixtures"
MIC_RATE = 44_100
FADE_IN_S = 0.15
FADE_OUT_S = 0.2
GAP_S = 0.4
BACKGROUNDS = {"dark": (32, 33, 36), "light": (241, 243, 244)}
VIDEO_SIZE = (800, 240)


def load_fixture(name: str, seconds: float | None = None) -> np.ndarray:
    with wave.open(str(FIXTURES / f"{name}.wav")) as f:
        rate = f.getframerate()
        samples = np.frombuffer(f.readframes(f.getnframes()), np.int16).astype(np.float32) / 32768
    t_out = np.arange(round(len(samples) * MIC_RATE / rate)) / MIC_RATE
    audio = np.interp(t_out, np.arange(len(samples)) / rate, samples).astype(np.float32)
    return audio[: round(seconds * MIC_RATE)] if seconds else audio


@dataclass(frozen=True)
class Sequence:
    audio: np.ndarray
    hands_free: bool
    after: list[tuple[Phase, float]]
    samples_s: list[float]


def timeline() -> list[Sequence]:
    return [
        Sequence(
            load_fixture("en"),
            False,
            [(Phase.TRANSCRIBING, 0.45), (Phase.POLISHING, 2.2), (Phase.DONE, BEAT_S)],
            [0.05, 0.5, 1.5, 2.5, 3.5, 4.5, 5.5, 6.05, 6.3, 6.7, 7.5, 8.4, 8.75, 8.95, 9.2],
        ),
        Sequence(
            load_fixture("pt", 3.0),
            True,
            [(Phase.CANCELLED, BEAT_S)],
            [0.5, 1.5, 2.5, 3.1, 3.3],
        ),
        Sequence(
            np.zeros(round(1.2 * MIC_RATE), np.float32),
            False,
            [(Phase.TRANSCRIBING, 0.3), (Phase.FAILED, BEAT_S)],
            [0.3, 1.0, 1.35, 1.6, 1.8],
        ),
    ]


def frames(scale: float):
    """Yield (frame, label or None) at FPS for the whole timeline."""
    meter = LevelMeter(MIC_RATE)
    hop = MIC_RATE // FPS
    for n, seq in enumerate(timeline(), 1):
        meter.reset()
        record_s = len(seq.audio) / MIC_RATE
        phases = [(Phase.RECORDING, record_s), *seq.after]
        total = sum(d for _, d in phases) + FADE_OUT_S
        pending = sorted(seq.samples_s)
        t, start = 0.0, 0.0
        for index, (phase, duration) in enumerate(phases):
            last = index == len(phases) - 1
            end = start + duration + (FADE_OUT_S if last else 0)
            while t < end - 1e-9:
                if phase is Phase.RECORDING:
                    i = round(t * MIC_RATE)
                    meter.feed(seq.audio[i : i + hop])
                presence = min(1.0, t / FADE_IN_S, max(0.0, (total - t) / FADE_OUT_S))
                levels, spectrum = meter.snapshot()
                frame = OverlayFrame(
                    phase=phase,
                    phase_elapsed=t - start,
                    presence=presence,
                    hands_free=seq.hands_free,
                    recording_elapsed=min(t, record_s),
                    levels=levels,
                    spectrum=spectrum,
                    scale=scale,
                )
                label = None
                if pending and t >= pending[0] - 1e-9:
                    pending.pop(0)
                    label = f"{n}:{phase.value} +{t - start:.2f}s p={presence:.2f}"
                yield frame, label
                t += 1 / FPS
            start += duration
        for _ in range(round(GAP_S * FPS)):
            frame = OverlayFrame(Phase.DONE, BEAT_S + FADE_OUT_S, 0.0, False, 0.0, *meter.snapshot(), scale=scale)
            yield frame, None


def composite(image: Image.Image, background: tuple[int, int, int], size: tuple[int, int]) -> Image.Image:
    canvas = Image.new("RGB", size, background)
    canvas.paste(image, ((size[0] - image.width) // 2, (size[1] - image.height) // 2), image)
    return canvas


def contact_sheet(samples: list[tuple[str, Image.Image]], background: tuple[int, int, int]) -> Image.Image:
    font = ImageFont.load_default(size=13)
    cell_w, cell_h = samples[0][1].width + 20, samples[0][1].height + 34
    cols = 3
    rows = -(-len(samples) // cols)
    sheet = Image.new("RGB", (cols * cell_w, rows * cell_h), background)
    text = (230, 230, 230) if sum(background) < 384 else (40, 40, 40)
    draw = ImageDraw.Draw(sheet)
    for i, (label, image) in enumerate(samples):
        x, y = (i % cols) * cell_w, (i // cols) * cell_h
        draw.text((x + 10, y + 6), label, fill=text, font=font)
        sheet.paste(image, (x + 10, y + 26), image)
    return sheet


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--renderer", default="parakeet_dictate.render:render", help="module:callable")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--scale", type=float, default=1.0)
    args = parser.parse_args()

    module, _, attr = args.renderer.partition(":")
    render = getattr(importlib.import_module(module), attr)
    args.out.mkdir(parents=True, exist_ok=True)

    ffmpeg = None
    if shutil.which("ffmpeg"):
        ffmpeg = subprocess.Popen(
            ["ffmpeg", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24",
             "-s", f"{VIDEO_SIZE[0]}x{VIDEO_SIZE[1]}", "-r", str(FPS), "-i", "-",
             "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "16", str(args.out / "video.mp4")],
            stdin=subprocess.PIPE,
        )

    times, samples, problems = [], [], []
    for frame, label in frames(args.scale):
        t0 = time.perf_counter()
        image = render(frame)
        times.append((time.perf_counter() - t0) * 1000)
        if image.mode != "RGBA" or image.size != frame.size:
            problems.append(f"{frame.phase.value} +{frame.phase_elapsed:.2f}s: got {image.mode} {image.size}")
            image = image.convert("RGBA").resize(frame.size)
        if frame.presence == 0 and image.getchannel("A").getextrema()[1] > 0:
            problems.append(f"{frame.phase.value} +{frame.phase_elapsed:.2f}s: visible pixels at presence 0")
        if label:
            samples.append((label, image))
        if ffmpeg:
            ffmpeg.stdin.write(composite(image, BACKGROUNDS["dark"], VIDEO_SIZE).tobytes())
    if ffmpeg:
        ffmpeg.stdin.close()
        ffmpeg.wait()

    for name, background in BACKGROUNDS.items():
        contact_sheet(samples, background).save(args.out / f"contact_{name}.png")
    ms = np.array(times)
    report = (
        f"frames {len(ms)}  render ms p50 {np.percentile(ms, 50):.2f}  p95 {np.percentile(ms, 95):.2f}"
        f"  max {ms.max():.2f}\n" + "".join(f"problem: {p}\n" for p in dict.fromkeys(problems))
    )
    (args.out / "timing.txt").write_text(report, encoding="utf-8")
    print(report, end="")
    print(f"wrote {args.out}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
