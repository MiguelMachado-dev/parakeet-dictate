"""Record ~2 s from the default mic through the app's Recorder and report what came back."""

import sys
import time

import numpy as np

from parakeet_dictate.recorder import Recorder

SECONDS = 2.0


def main() -> int:
    recorder = Recorder()
    t0 = time.perf_counter()
    recorder.start()
    open_ms = (time.perf_counter() - t0) * 1000
    time.sleep(SECONDS)
    audio = recorder.stop()

    rms = float(np.sqrt(np.mean(np.square(audio.samples))))
    print(f"device:      {recorder.device_name}")
    print(f"sample rate: {audio.rate} Hz")
    print(f"frames:      {len(audio.samples)} ({audio.seconds:.2f} s)")
    print(f"rms:         {rms:.5f} ({20 * np.log10(max(rms, 1e-10)):.1f} dBFS)")
    print(f"open time:   {open_ms:.0f} ms (PortAudio re-init + stream start)")
    return 0 if len(audio.samples) > 0.5 * SECONDS * audio.rate else 1


if __name__ == "__main__":
    sys.exit(main())
