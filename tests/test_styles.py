import numpy as np
import pytest

from parakeet_dictate.meter import LEVEL_HISTORY, SPECTRUM_BANDS
from parakeet_dictate.overlay_frame import BEAT_S, OverlayFrame, Phase
from parakeet_dictate.styles import DEFAULT_STYLE, STYLES

rng = np.random.default_rng(7)
LOUD = rng.uniform(0.3, 1.0, LEVEL_HISTORY).astype(np.float32), rng.uniform(0.2, 1.0, SPECTRUM_BANDS).astype(np.float32)
SILENT = np.zeros(LEVEL_HISTORY, np.float32), np.zeros(SPECTRUM_BANDS, np.float32)


def frame(phase: Phase, presence: float, scale: float, hands_free: bool = False, audio=LOUD) -> OverlayFrame:
    elapsed = BEAT_S / 2 if phase in (Phase.DONE, Phase.CANCELLED, Phase.FAILED) else 0.7
    return OverlayFrame(phase, elapsed, presence, hands_free, 2.0, *audio, scale=scale)


def test_the_default_style_is_registered():
    assert DEFAULT_STYLE in STYLES


@pytest.mark.parametrize("scale", [1.0, 1.5])
@pytest.mark.parametrize("phase", list(Phase))
@pytest.mark.parametrize("style", list(STYLES))
def test_every_phase_draws_inside_the_canvas(style, phase, scale):
    f = frame(phase, 1.0, scale, hands_free=phase is Phase.RECORDING)
    image = STYLES[style](f)
    assert image.mode == "RGBA" and image.size == f.size
    alpha = np.asarray(image.getchannel("A"))
    assert alpha.max() > 200, "the body is visible"
    border = np.concatenate((alpha[0], alpha[-1], alpha[:, 0], alpha[:, -1]))
    assert border.max() == 0, "nothing reaches the canvas edge"


@pytest.mark.parametrize("style", list(STYLES))
def test_presence_zero_is_fully_transparent(style):
    image = STYLES[style](frame(Phase.RECORDING, 0.0, 1.0))
    assert image.getchannel("A").getextrema() == (0, 0)


@pytest.mark.parametrize("style", list(STYLES))
def test_recording_reacts_to_the_voice(style):
    loud = np.asarray(STYLES[style](frame(Phase.RECORDING, 1.0, 1.0)), np.int16)
    silent = np.asarray(STYLES[style](frame(Phase.RECORDING, 1.0, 1.0, audio=SILENT)), np.int16)
    assert np.abs(loud - silent).mean() > 1.0
