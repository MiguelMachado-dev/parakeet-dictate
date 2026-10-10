import numpy as np
import pytest

from parakeet_dictate.meter import CURVE, HOP_S, LEVEL_HISTORY, SPECTRUM_BANDS, LevelMeter

RATE = 44_100


def tone(db: float, seconds: float, hz: float = 440.0) -> np.ndarray:
    t = np.arange(round(RATE * seconds)) / RATE
    amplitude = np.sqrt(2) * 10 ** (db / 20)
    return (amplitude * np.sin(2 * np.pi * hz * t)).astype(np.float32)


def test_silence_reads_zero():
    meter = LevelMeter(RATE)
    meter.feed(np.zeros(RATE, np.float32))
    levels, spectrum = meter.snapshot()
    assert levels.shape == (LEVEL_HISTORY,) and spectrum.shape == (SPECTRUM_BANDS,)
    assert levels.max() == 0 and spectrum.max() == 0


def test_room_noise_below_the_floor_reads_zero():
    meter = LevelMeter(RATE)
    meter.feed(tone(-87, 1.0))
    assert meter.snapshot()[0].max() == 0


def test_a_quiet_mic_still_fills_most_of_the_scale():
    meter = LevelMeter(RATE)
    meter.feed(tone(-40, 1.0))
    assert meter.snapshot()[0][-1] == pytest.approx((20 / 25) ** CURVE, abs=0.02)


def test_a_loud_mic_moves_the_ceiling_up_to_its_peak():
    meter = LevelMeter(RATE)
    meter.feed(tone(-12, 1.0))
    assert meter.snapshot()[0][-1] == pytest.approx(1.0, abs=0.02)


def test_history_is_oldest_first():
    meter = LevelMeter(RATE)
    meter.feed(np.zeros(RATE, np.float32))
    meter.feed(tone(-20, 10 * HOP_S))
    levels = meter.snapshot()[0]
    assert levels[0] == 0 and levels[-1] > 0.5


def test_the_tone_lands_in_its_band():
    meter = LevelMeter(RATE)
    meter.feed(tone(-20, 0.5, hz=1000))
    edges = np.geomspace(80, 8000, SPECTRUM_BANDS + 1)
    assert int(np.argmax(meter.snapshot()[1])) == int(np.digitize(1000, edges) - 1)


def test_feeding_in_odd_block_sizes_matches_one_block():
    signal = tone(-25, 0.7)
    whole, pieces = LevelMeter(RATE), LevelMeter(RATE)
    whole.feed(signal)
    for chunk in np.array_split(signal, 37):
        pieces.feed(chunk)
    np.testing.assert_allclose(whole.snapshot()[0], pieces.snapshot()[0], atol=1e-6)


def test_reset_clears_history():
    meter = LevelMeter(RATE)
    meter.feed(tone(-20, 0.5))
    meter.reset()
    assert meter.snapshot()[0].max() == 0
