"""Exploratory: does OpenBLAS's thread count change onnx_asr's numpy mel features for the CPU provider?

usage: mel_features.py OUT.npy   (set OPENBLAS_NUM_THREADS in the environment, or leave it unset)
"""

import sys
import time
import wave
from pathlib import Path

import numpy as np

sys.path.insert(0, r"C:\Users\migue\.t3\worktrees\parakeet-dictate\t3-073be6d8\scripts")
import bench_resources as b  # noqa: E402
from onnx_asr.preprocessors.numpy_preprocessor import NemoPreprocessorNumpy  # noqa: E402


def read_fixture(name):
    with wave.open(str(b.FIXTURES / f"{name}.wav")) as f:
        return np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768


clips = b.build_clips(np, read_fixture)
pre = NemoPreprocessorNumpy("nemo128")
out = {}
for name in ("en", "long", "xlong"):
    samples = clips[name]
    n_out = round(len(samples) * 16_000 / b.RATE)
    x = (np.fft.irfft(np.fft.rfft(samples)[: n_out // 2 + 1], n_out) * (n_out / len(samples))).astype(np.float32)
    t0 = time.perf_counter()
    features, lens = pre(x[None, :], np.array([len(x)], np.int64))
    out[name] = features
    print(name, features.shape, f"{(time.perf_counter() - t0) * 1000:.1f} ms", flush=True)
np.savez(sys.argv[1], **out)
