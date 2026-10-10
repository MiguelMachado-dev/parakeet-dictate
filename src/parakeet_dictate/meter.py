"""Live microphone level and spectrum for the overlay, fed from the audio callback."""

import threading

import numpy as np
import numpy.typing as npt

HOP_S = 0.02
LEVEL_HISTORY = 96
SPECTRUM_BANDS = 24
FFT_SIZE = 2048
BAND_RANGE_HZ = (80.0, 8000.0)
FLOOR_DB = -60.0
# The ceiling follows the loudest recent hop, but never drops below this, so a quiet mic still
# fills the scale and room noise does not.
MIN_CEILING_DB = -35.0
PEAK_DECAY_DB_PER_S = 3.0
# Speech sits in the top quarter of the dB range (fixture median -21 dBFS, peak -13), so a
# linear map pins the bars near full. The curve spreads that quarter over most of the scale.
CURVE = 2.5
ATTACK = 0.6
RELEASE = 0.15
EPS = 1e-12


def _db(power: float | npt.NDArray[np.float64]) -> npt.NDArray[np.float64]:
    return 10.0 * np.log10(np.asarray(power) + EPS)


class LevelMeter:
    """Single writer (the audio callback) and single reader (the overlay thread)."""

    def __init__(self, rate: int) -> None:
        self.rate = rate
        self._hop = max(1, round(rate * HOP_S))
        self._window = np.hanning(FFT_SIZE).astype(np.float32)
        freqs = np.fft.rfftfreq(FFT_SIZE, 1.0 / rate)
        edges = np.geomspace(*BAND_RANGE_HZ, SPECTRUM_BANDS + 1)
        self._band_of_bin = np.digitize(freqs, edges) - 1
        self._lock = threading.Lock()
        self.reset()

    def reset(self) -> None:
        with self._lock:
            self._levels = np.zeros(LEVEL_HISTORY, np.float32)
            self._spectrum = np.zeros(SPECTRUM_BANDS, np.float32)
            self._tail = np.zeros(FFT_SIZE, np.float32)
            self._pending = 0
            self._level = 0.0
            self._peak_db = MIN_CEILING_DB

    def feed(self, block: npt.NDArray[np.float32]) -> None:
        block = np.asarray(block, np.float32).reshape(-1)
        with self._lock:
            while len(block):
                take = min(len(block), self._hop - self._pending)
                self._tail = np.concatenate((self._tail[take:], block[:take]))
                self._pending += take
                block = block[take:]
                if self._pending == self._hop:
                    self._pending = 0
                    self._analyze_hop()

    def snapshot(self) -> tuple[npt.NDArray[np.float32], npt.NDArray[np.float32]]:
        with self._lock:
            return self._levels.copy(), self._spectrum.copy()

    def _analyze_hop(self) -> None:
        hop = self._tail[-self._hop :]
        rms_db = float(_db(np.mean(hop.astype(np.float64) ** 2)))
        self._peak_db = max(self._peak_db - PEAK_DECAY_DB_PER_S * HOP_S, rms_db)
        ceiling = max(MIN_CEILING_DB, self._peak_db)

        target = float(np.clip((rms_db - FLOOR_DB) / (ceiling - FLOOR_DB), 0.0, 1.0)) ** CURVE
        self._level += (target - self._level) * (ATTACK if target > self._level else RELEASE)
        self._levels = np.roll(self._levels, -1)
        self._levels[-1] = self._level

        # Scaled so the band powers sum to the frame's mean power, which keeps bands on the
        # same dB scale as the level.
        spectrum = np.abs(np.fft.rfft(self._tail * self._window)) ** 2
        spectrum *= 2.0 / (FFT_SIZE * np.sum(self._window**2))
        valid = (self._band_of_bin >= 0) & (self._band_of_bin < SPECTRUM_BANDS)
        bands = np.bincount(self._band_of_bin[valid], spectrum[valid], SPECTRUM_BANDS)
        band_target = (np.clip((_db(bands) - FLOOR_DB) / (ceiling - FLOOR_DB), 0.0, 1.0) ** CURVE).astype(np.float32)
        rate = np.where(band_target > self._spectrum, ATTACK, RELEASE).astype(np.float32)
        self._spectrum += (band_target - self._spectrum) * rate
