"""Exploratory: what does the first transcription at a microphone rate the warm-up did not use cost?

Warms up at 44.1 kHz like the app, then transcribes 5 s of noise at 48 kHz three times and at 32 kHz
twice. Both rates resample to the same 80,000 samples, so the first 32 kHz call meets no new encoder
length: it differs from the second only by its resampler session's first use. Prints one JSON line.
"""

import json
import time
from pathlib import Path

import parakeet_dictate.app  # noqa: F401
import numpy as np
from parakeet_dictate.recorder import Audio
from parakeet_dictate.transcriber import load_transcriber

MODEL = Path(r"C:\Users\migue\www\parakeet-dictate\models\parakeet-tdt-0.6b-v3")
t = load_transcriber(MODEL, "cuda")
t.warm_up(44_100)
noise = np.random.default_rng(0).normal(0, 0.01, 48_000 * 6).astype(np.float32)
calls = []
for rate in (48_000, 48_000, 48_000, 32_000, 32_000):
    t0 = time.perf_counter()
    t.transcribe(Audio(noise[: rate * 5], rate))
    calls.append({"rate": rate, "ms": round((time.perf_counter() - t0) * 1000, 1)})
print(json.dumps({"source": str(Path(parakeet_dictate.__file__).parent), "calls": calls}))
