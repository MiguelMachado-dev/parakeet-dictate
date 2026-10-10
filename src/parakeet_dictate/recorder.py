from dataclasses import dataclass

import numpy as np
import numpy.typing as npt
import sounddevice as sd

from .meter import LevelMeter


@dataclass(frozen=True)
class Audio:
    samples: npt.NDArray[np.float32]
    rate: int

    @property
    def seconds(self) -> float:
        return len(self.samples) / self.rate


class RecorderError(RuntimeError):
    pass


def default_input_rate() -> int:
    try:
        return int(sd.query_devices(kind="input")["default_samplerate"])
    except sd.PortAudioError:
        return 16_000


class Recorder:
    def __init__(self) -> None:
        self._stream: sd.InputStream | None = None
        self._chunks: list[npt.NDArray[np.float32]] = []
        self.device_name = ""
        # Replaced on each start, so the overlay always reads levels of the current recording
        # at the rate its stream actually runs.
        self.meter = LevelMeter(16_000)

    def start(self) -> None:
        self.discard()
        # PortAudio snapshots the device list at init. Re-init so a headset connected
        # after launch is picked up as the default (~50 ms).
        sd._terminate()
        sd._initialize()
        device = sd.query_devices(kind="input")
        stream = sd.InputStream(
            samplerate=device["default_samplerate"], channels=1, dtype="float32", callback=self._on_audio
        )
        self.meter = LevelMeter(int(stream.samplerate))
        stream.start()
        self._stream, self.device_name = stream, device["name"]

    def stop(self) -> Audio:
        stream = self._stream
        if stream is None:
            raise RecorderError("stop() without start()")
        self._stream = None
        try:
            stream.stop()
        finally:
            stream.close()
        chunks, self._chunks = self._chunks, []
        if not chunks:
            raise RecorderError(f"no audio from {self.device_name!r}")
        return Audio(np.concatenate(chunks), int(stream.samplerate))

    def discard(self) -> None:
        stream, self._stream = self._stream, None
        if stream is not None:
            stream.close()
        self._chunks = []

    def _on_audio(self, indata: npt.NDArray[np.float32], frames: int, time: object, status: object) -> None:
        samples = indata[:, 0].copy()
        self._chunks.append(samples)
        self.meter.feed(samples)
