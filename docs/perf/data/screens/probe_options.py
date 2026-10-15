"""Exploratory screen (not the reported benchmark): load the CUDA model with one ONNX Runtime option set,
then report load/warm time, threads, memory, GPU memory, CPU time and transcripts.

usage: probe_options.py NAME   (NAME is a key of CONFIGS); prints one JSON line.
"""

import ctypes
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
CONFIGS = {
    "base": {},
    "t1": {"intra": 1},
    "nospin": {"entries": {"session.intra_op.allow_spinning": "0"}},
    "t1_initdev": {"intra": 1, "entries": {"session.use_device_allocator_for_initializers": "1"}},
    "t1_same": {"intra": 1, "cuda": {"arena_extend_strategy": "kSameAsRequested"}},
    "t1_heur": {"intra": 1, "cuda": {"cudnn_conv_algo_search": "HEURISTIC"}},
    "t1_cpures": {"intra": 1, "resampler_cpu": True},
    "t1_mem": {"intra": 1, "entries": {"session.use_device_allocator_for_initializers": "1"},
               "cuda": {"arena_extend_strategy": "kSameAsRequested"}},
    "t1_default": {"intra": 1, "cuda": {"cudnn_conv_algo_search": "DEFAULT"}},
    "t1_nopattern": {"intra": 1, "mem_pattern": False},
    "t1_heur_nopattern": {"intra": 1, "mem_pattern": False, "cuda": {"cudnn_conv_algo_search": "HEURISTIC"}},
    "nopattern": {"mem_pattern": False},
    "t1_same_cpures": {"intra": 1, "resampler_cpu": True, "cuda": {"arena_extend_strategy": "kSameAsRequested"}},
}
k32 = ctypes.WinDLL("kernel32")
pid = k32.GetCurrentProcessId()


def snap(label):
    m = winstats.memory(pid)
    g = gpu.read(pid)
    return {"label": label, "ws": round(m["working_set_mb"]), "pws": round(m["private_working_set_mb"]),
            "commit": round(m["commit_mb"]), "cpu_s": round(m["cpu_s"], 2), "threads": winstats.threads(pid),
            "gpu": round(g["gpu_dedicated_mb"])}


def read_fixture(name):
    import wave
    with wave.open(str(b.FIXTURES / f"{name}.wav")) as f:
        return np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768


cfg = CONFIGS[sys.argv[1]]
opts = rt.SessionOptions()
if "intra" in cfg:
    opts.intra_op_num_threads = cfg["intra"]
if "mem_pattern" in cfg:
    opts.enable_mem_pattern = cfg["mem_pattern"]
for key, value in cfg.get("entries", {}).items():
    opts.add_session_config_entry(key, value)
rt.set_default_logger_severity(3)
snaps = [snap("imported")]
t0 = time.perf_counter()
rt.preload_dlls()
resampler_config = {"sess_options": opts, "providers": ["CPUExecutionProvider"]} if cfg.get("resampler_cpu") else None
model = onnx_asr.load_model(MODEL_NAME, MODEL, quantization=None, sess_options=opts,
                            providers=[("CUDAExecutionProvider", cfg.get("cuda", {})), "CPUExecutionProvider"],
                            resampler_config=resampler_config)
t = Transcriber(model, model.asr._encoder.get_providers()[0], None)
load_s = time.perf_counter() - t0
snaps.append(snap("loaded"))
t0 = time.perf_counter()
t.warm_up(44_100)
warm_s = time.perf_counter() - t0
snaps.append(snap("warm"))

clips = b.build_clips(np, read_fixture)
rng = np.random.default_rng(7)
rows = []
for i, name in enumerate(["en", "short", "en", "mid", "en", "pt", "en", "long", "en", "xlong", "en", "mid"]):
    samples = np.concatenate([clips[name], (rng.standard_normal(round((0.05 + 0.0137 * i) * 44_100)) * 3e-4).astype(np.float32)])
    c0 = winstats.memory(pid)["cpu_s"]
    t0 = time.perf_counter()
    text = t.transcribe(Audio(samples, 44_100))
    rows.append({"clip": name, "ms": round((time.perf_counter() - t0) * 1000, 1),
                 "cpu_ms": round((winstats.memory(pid)["cpu_s"] - c0) * 1000), "text": text})
same = []
for _ in range(6):
    t0 = time.perf_counter()
    t.transcribe(Audio(clips["en"], 44_100))
    same.append(round((time.perf_counter() - t0) * 1000, 1))
snaps.append(snap("after"))
time.sleep(5)
snaps.append(snap("idle5"))
print(json.dumps({"config": sys.argv[1], "load_s": round(load_s, 2), "warm_s": round(warm_s, 2), "snaps": snaps,
                  "rows": rows, "same_en_ms": same}))
