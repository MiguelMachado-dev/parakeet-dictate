"""Exploratory: with the code held fixed, does OpenBLAS's thread count alone change a CPU-provider transcript?

Rebuilds the stages session's new-length inputs exactly (bench_resources' fresh(): seed 11, tail i+1 for
call i), loads the int8 model on the CPU provider and transcribes the two calls that differed between sides.
usage: OPENBLAS_NUM_THREADS=N openblas_transcripts.py    prints one JSON line
"""

import json
import os
import sys
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "scripts"))
import parakeet_dictate.app  # noqa: E402,F401  the package first, as in the app
import numpy as np  # noqa: E402
import bench_resources as b  # noqa: E402
from parakeet_dictate.recorder import Audio  # noqa: E402
from parakeet_dictate.transcriber import load_transcriber  # noqa: E402

MODEL = Path(r"C:\Users\migue\www\parakeet-dictate\models\parakeet-tdt-0.6b-v3")


def read_fixture(name):
    with wave.open(str(b.FIXTURES / f"{name}.wav")) as f:
        return np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768


clips = b.build_clips(np, read_fixture)
tails = iter(range(1, 1_000_000))
rng = np.random.default_rng(11)


def fresh(name):
    tail = (rng.standard_normal(round((0.05 + 0.0137 * next(tails)) * b.RATE)) * b.NOISE).astype(np.float32)
    return np.concatenate([clips[name], tail])


inputs = [(name, fresh(name)) for _ in range(3) for name in b.CLIPS]
model = load_transcriber(MODEL, "cpu")
model.warm_up(44_100)
texts = {i: {"clip": inputs[i][0], "text": model.transcribe(Audio(inputs[i][1], b.RATE))} for i in (10, 17)}
print(json.dumps({"openblas_threads": os.environ.get("OPENBLAS_NUM_THREADS"), "source": parakeet_dictate.__file__,
                  "calls": texts}, ensure_ascii=False))
