"""Renders reference frames with the Windows app's overlay styles, for the Swift port to match pixel for pixel.

    uv run --no-project --python 3.12 --with numpy --with pillow python mac/scripts/overlay_reference.py /tmp/overlay-ref
    OVERLAY_REFERENCE_DIR=/tmp/overlay-ref swift test --package-path mac --filter OverlayReference

Writes frames.json, which lists every frame's inputs and its file, and one raw straight-alpha RGBA file per frame.
"""

import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from parakeet_dictate.overlay_frame import BEAT_S, OverlayFrame, Phase  # noqa: E402
from parakeet_dictate.styles import STYLES  # noqa: E402

LEVELS, BANDS = 96, 24


def audio(kind: str) -> tuple[np.ndarray, np.ndarray]:
    i = np.arange(LEVELS, dtype=np.float32)
    b = np.arange(BANDS, dtype=np.float32)
    if kind == "silent":
        return np.zeros(LEVELS, np.float32), np.zeros(BANDS, np.float32)
    if kind == "loud":
        rng = np.random.default_rng(7)
        return rng.uniform(0.3, 1.0, LEVELS).astype(np.float32), rng.uniform(0.2, 1.0, BANDS).astype(np.float32)
    # Speech-like: syllables in the level history and energy in the voice bands.
    levels = np.clip(0.5 + 0.45 * np.sin(i * 0.37) * np.sin(i * 0.11 + 1.0), 0, 1).astype(np.float32)
    spectrum = np.clip(np.exp(-(((b - 8) / 5.0) ** 2)) * 0.9 + 0.05 * np.cos(b), 0, 1).astype(np.float32)
    return levels, spectrum


def cases():
    beat_times = [0.0, 0.05, 0.12, 0.2, 0.3, BEAT_S / 2, BEAT_S, 0.6]
    for phase in Phase:
        ending = phase in (Phase.DONE, Phase.CANCELLED, Phase.FAILED)
        for elapsed in beat_times if ending else [0.0, 0.04, 0.1, 0.3, 0.7, 1.3, 2.9]:
            yield phase, elapsed, 1.0, False, "speech", 1.0
        yield phase, 0.7 if not ending else BEAT_S / 2, 1.0, True, "loud", 1.0
        yield phase, 0.7 if not ending else BEAT_S / 2, 1.0, False, "silent", 1.0
        yield phase, 0.5 if not ending else 0.1, 1.0, phase is Phase.RECORDING, "speech", 2.0
    for presence in [0.0, 0.05, 0.15, 0.3, 0.5, 0.8]:
        yield Phase.RECORDING, 0.2, presence, False, "speech", 1.0
        yield Phase.DONE, 0.4, presence, False, "speech", 1.0
    yield Phase.RECORDING, 0.3, 0.6, True, "loud", 1.5


def main() -> None:
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    entries = []
    for style, render in STYLES.items():
        for n, (phase, elapsed, presence, hands_free, kind, scale) in enumerate(cases()):
            levels, spectrum = audio(kind)
            frame = OverlayFrame(phase, elapsed, presence, hands_free, 2.0 + elapsed, levels, spectrum, scale=scale)
            image = render(frame)
            name = f"{style}-{n:03d}.rgba"
            (out / name).write_bytes(np.asarray(image, np.uint8).tobytes())
            entries.append({
                "style": style, "file": name, "phase": phase.value, "phase_elapsed": elapsed,
                "presence": presence, "hands_free": hands_free, "recording_elapsed": 2.0 + elapsed,
                "levels": [float(v) for v in levels], "spectrum": [float(v) for v in spectrum],
                "scale": scale, "width": image.width, "height": image.height,
            })
    (out / "frames.json").write_text(json.dumps(entries))
    print(f"{len(entries)} frames in {out}")


if __name__ == "__main__":
    main()
