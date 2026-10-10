import sys
import time

import numpy as np

from parakeet_dictate.recorder import Recorder

SECONDS = 2.0


def main() -> int:
    recorder = Recorder()
    callback_us: list[float] = []
    on_audio = recorder._on_audio

    def timed_on_audio(*args: object) -> None:
        t0 = time.perf_counter()
        on_audio(*args)
        callback_us.append((time.perf_counter() - t0) * 1e6)

    recorder._on_audio = timed_on_audio
    t0 = time.perf_counter()
    recorder.start()
    open_ms = (time.perf_counter() - t0) * 1000
    time.sleep(SECONDS)
    audio = recorder.stop()

    rms = float(np.sqrt(np.mean(np.square(audio.samples))))
    levels, spectrum = recorder.meter.snapshot()
    us = np.array(callback_us)
    print(f"device:      {recorder.device_name}")
    print(f"sample rate: {audio.rate} Hz")
    print(f"frames:      {len(audio.samples)} ({audio.seconds:.2f} s)")
    print(f"rms:         {rms:.5f} ({20 * np.log10(max(rms, 1e-10)):.1f} dBFS)")
    print(f"open time:   {open_ms:.0f} ms (PortAudio re-init + stream start)")
    print(f"meter:       {recorder.meter.rate} Hz, level max {levels.max():.3f}, spectrum max {spectrum.max():.3f}")
    print(f"callback:    {len(us)} blocks, p50 {np.percentile(us, 50):.0f} us, max {us.max():.0f} us (copy + meter)")
    return 0 if len(audio.samples) > 0.5 * SECONDS * audio.rate and recorder.meter.rate == audio.rate else 1


if __name__ == "__main__":
    sys.exit(main())
