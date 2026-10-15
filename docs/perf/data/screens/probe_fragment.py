"""Exploratory: does kSameAsRequested fragment the CUDA arena over many distinct lengths?

usage: probe_fragment.py ARENA   ARENA in default, same; prints one JSON line.
"""

import json
import sys
import time
from pathlib import Path

sys.path.insert(0, r"C:\Users\migue\.t3\worktrees\parakeet-dictate\t3-073be6d8\scripts")
import winstats  # noqa: E402

gpu = winstats.GpuMemory()
import parakeet_dictate.app  # noqa: F401,E402
import numpy as np  # noqa: E402
import onnx_asr  # noqa: E402
import onnxruntime as rt  # noqa: E402
import bench_resources as b  # noqa: E402
from parakeet_dictate.recorder import Audio  # noqa: E402
from parakeet_dictate.transcriber import MODEL_NAME, Transcriber  # noqa: E402

MODEL = Path(r"C:\Users\migue\www\parakeet-dictate\models\parakeet-tdt-0.6b-v3")
cuda = {"arena_extend_strategy": "kSameAsRequested"} if sys.argv[1] == "same" else {}
opts = rt.SessionOptions()
opts.intra_op_num_threads = 1
if sys.argv[1] == "initdev":
    opts.add_session_config_entry("session.use_device_allocator_for_initializers", "1")
rt.set_default_logger_severity(3)
rt.preload_dlls()
t_load = time.perf_counter()
if sys.argv[1] == "lazyres":
    sys.path.insert(0, str(Path(__file__).parent))
    from o1_draft import _Manager
    model = _Manager(opts, [("CUDAExecutionProvider", cuda), "CPUExecutionProvider"]).create_asr(MODEL_NAME, MODEL, quantization=None)
else:
    model = onnx_asr.load_model(MODEL_NAME, MODEL, quantization=None, sess_options=opts,
                                providers=[("CUDAExecutionProvider", cuda), "CPUExecutionProvider"])
load_s = time.perf_counter() - t_load
loaded_gpu = round(gpu.read()["gpu_dedicated_mb"])
t = Transcriber(model, model.asr._encoder.get_providers()[0], None)
t.warm_up(44_100)


def read_fixture(name):
    import wave
    with wave.open(str(b.FIXTURES / f"{name}.wav")) as f:
        return np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768


speech = b.build_clips(np, read_fixture)["xlong"]
rng = np.random.default_rng(11)
lengths = rng.uniform(0.5, 60.0, 150)
trace, ms = [], []
for i, seconds in enumerate(lengths):
    t0 = time.perf_counter()
    t.transcribe(Audio(speech[: int(seconds * 44_100)], 44_100))
    ms.append(round((time.perf_counter() - t0) * 1000, 1))
    if i % 10 == 9:
        trace.append(round(gpu.read()["gpu_dedicated_mb"]))
print(json.dumps({"arena": sys.argv[1], "load_s": round(load_s, 2), "loaded_gpu": loaded_gpu, "threads": winstats.threads(), "gpu_trace": trace, "ms_sum": round(sum(ms)), "ms": ms}))
