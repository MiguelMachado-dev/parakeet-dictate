"""Exploratory: does intra_op_num_threads=1 move transcription onto E-cores, and does tagging the
transcribing thread HighQoS (SetThreadInformation ThreadPowerThrottling, execution speed off) fix it?

usage: probe_qos.py CONFIG   CONFIG in base, t1, t1_qos, base_qos; prints one JSON line.
"""

import ctypes
import json
import sys
import time
from ctypes import wintypes
from pathlib import Path

sys.path.insert(0, r"C:\Users\migue\.t3\worktrees\parakeet-dictate\t3-073be6d8\scripts")
import parakeet_dictate.app  # noqa: F401,E402
import numpy as np  # noqa: E402
import onnx_asr  # noqa: E402
import onnxruntime as rt  # noqa: E402
import bench_resources as b  # noqa: E402
from parakeet_dictate.recorder import Audio  # noqa: E402
from parakeet_dictate.transcriber import MODEL_NAME, Transcriber  # noqa: E402

MODEL = Path(r"C:\Users\migue\www\parakeet-dictate\models\parakeet-tdt-0.6b-v3")
k32 = ctypes.WinDLL("kernel32", use_last_error=True)


class THREAD_POWER_THROTTLING_STATE(ctypes.Structure):
    _fields_ = [("Version", wintypes.ULONG), ("ControlMask", wintypes.ULONG), ("StateMask", wintypes.ULONG)]


k32.SetThreadInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
k32.GetCurrentThread.restype = wintypes.HANDLE

config = sys.argv[1]
if config.endswith("_qos"):
    state = THREAD_POWER_THROTTLING_STATE(1, 1, 0)
    assert k32.SetThreadInformation(k32.GetCurrentThread(), 3, ctypes.byref(state), ctypes.sizeof(state)), ctypes.get_last_error()
opts = rt.SessionOptions()
if config.startswith("t1"):
    opts.intra_op_num_threads = 1
rt.set_default_logger_severity(3)
rt.preload_dlls()
model = onnx_asr.load_model(MODEL_NAME, MODEL, quantization=None, sess_options=opts,
                            providers=[("CUDAExecutionProvider", {}), "CPUExecutionProvider"])
t = Transcriber(model, model.asr._encoder.get_providers()[0], None)
t.warm_up(44_100)


def read_fixture(name):
    import wave
    with wave.open(str(b.FIXTURES / f"{name}.wav")) as f:
        return np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768


clips = b.build_clips(np, read_fixture)
rng = np.random.default_rng(3)
rows = []
for cycle in range(3):
    for kind in ("fresh", "same"):
        for i in range(6):
            samples = clips["en"]
            if kind == "fresh":
                samples = np.concatenate([samples, (rng.standard_normal(round((0.05 + 0.0137 * (len(rows) + 1)) * 44_100)) * 3e-4).astype(np.float32)])
            c0 = k32.GetCurrentProcessorNumber()
            t0 = time.perf_counter()
            t.transcribe(Audio(samples, 44_100))
            ms = (time.perf_counter() - t0) * 1000
            rows.append({"kind": kind, "ms": round(ms, 1), "cores": [c0, k32.GetCurrentProcessorNumber()]})
print(json.dumps({"config": config, "rows": rows}))
