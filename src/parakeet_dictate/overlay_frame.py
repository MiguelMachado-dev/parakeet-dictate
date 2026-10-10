"""What the overlay draws on each frame. A renderer is a pure function of an OverlayFrame."""

from dataclasses import dataclass
from enum import Enum
from typing import Protocol

import numpy as np
import numpy.typing as npt
from PIL import Image

# Logical pixels at 100% scale. The renderer draws everything inside it, glow and shadow included,
# so the window never resizes.
CANVAS_SIZE = (400, 120)
FPS = 60
# How long the DONE, CANCELLED and FAILED beats play before the window fades out.
BEAT_S = 0.45


class Phase(Enum):
    RECORDING = "recording"
    TRANSCRIBING = "transcribing"
    POLISHING = "polishing"
    DONE = "done"
    CANCELLED = "cancelled"
    FAILED = "failed"


@dataclass(frozen=True)
class OverlayFrame:
    phase: Phase
    phase_elapsed: float
    # 0 is fully gone, 1 is fully shown. The window eases it on show and hide; the renderer
    # decides what partial presence looks like.
    presence: float
    hands_free: bool
    # Seconds since the recording began, frozen once it stops.
    recording_elapsed: float
    # meter.LEVEL_HISTORY values in 0..1, oldest first, one per meter hop.
    levels: npt.NDArray[np.float32]
    # meter.SPECTRUM_BANDS values in 0..1, low to high frequency.
    spectrum: npt.NDArray[np.float32]
    scale: float = 1.0

    @property
    def size(self) -> tuple[int, int]:
        return round(CANVAS_SIZE[0] * self.scale), round(CANVAS_SIZE[1] * self.scale)


class Renderer(Protocol):
    def __call__(self, frame: OverlayFrame) -> Image.Image:
        """Return a straight-alpha RGBA image of exactly frame.size."""
        ...
