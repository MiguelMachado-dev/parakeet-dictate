"""Exploratory: where does a GPU transcription's time go? Encoder vs decoder loop, CUDA vs CPU decoder."""

import ctypes
import sys
import time
from pathlib import Path

sys.path.insert(0, r"C:\Users\migue\.t3\worktrees\parakeet-dictate\t3-073be6d8\scripts")
import parakeet_dictate.app  # noqa: F401,E402
import numpy as np  # noqa: E402
import onnxruntime as rt  # noqa: E402
import bench_resources as b  # noqa: E402
from parakeet_dictate.transcriber import load_transcriber  # noqa: E402

k32 = ctypes.WinDLL("kernel32")
MODEL = Path(r"C:\Users\migue\www\parakeet-dictate\models\parakeet-tdt-0.6b-v3")
t = load_transcriber(MODEL, "cuda")
t.warm_up(44_100)
asr = t._model.asr


def read_fixture(name):
    import wave
    with wave.open(str(b.FIXTURES / f"{name}.wav")) as f:
        return np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768


clips = b.build_clips(np, read_fixture)
for name in ("en", "mid"):
    samples = clips[name]
    waveforms, lens = samples[None, :], np.array([len(samples)], np.int64)
    for _ in range(3):
        core = k32.GetCurrentProcessorNumber()
        t0 = time.perf_counter()
        resampled, rlens = t._model.resampler(waveforms, lens, 44_100)
        t1 = time.perf_counter()
        features, flens = asr._preprocessor(resampled, rlens)
        t2 = time.perf_counter()
        enc, enc_lens = asr._encode(features, flens)
        t3 = time.perf_counter()
        calls = [0]
        original = asr._decoder_joint.run

        def counting(*a, **kw):
            calls[0] += 1
            return original(*a, **kw)

        asr._decoder_joint.run = counting
        tokens = list(asr._decoding(enc, enc_lens))
        asr._decoder_joint.run = original
        t4 = time.perf_counter()
        print(f"{name} core {core}: resample {1000*(t1-t0):.1f} pre {1000*(t2-t1):.1f} enc {1000*(t3-t2):.1f} "
              f"decode {1000*(t4-t3):.1f} ms in {calls[0]} decoder calls ({1000*(t4-t3)/calls[0]:.3f} ms/call), "
              f"{len(tokens[0][0])} tokens", flush=True)

# Decoder per-call cost, CUDA session vs a CPU session of the same file.
cpu_opts = rt.SessionOptions()
cpu_dec = rt.InferenceSession(str(MODEL / "decoder_joint-model.onnx"), cpu_opts, providers=["CPUExecutionProvider"])
cpu1 = rt.SessionOptions()
cpu1.intra_op_num_threads = 1
cpu_dec1 = rt.InferenceSession(str(MODEL / "decoder_joint-model.onnx"), cpu1, providers=["CPUExecutionProvider"])
state = asr._create_state()
frame = enc.transpose(0, 2, 1)[0] if enc.shape[1] != 1024 else enc[0]
enc_t = enc[0]  # (T, 1024) after _encode's transpose
types = {i.name: i.type for i in asr._decoder_joint.get_inputs()}
print("decoder input types", types)
as_int = lambda v, name: np.array(v, np.int64 if "int64" in types[name] else np.int32)
feeds = {
    "encoder_outputs": enc_t[5][None, :, None],
    "targets": as_int([[asr._blank_idx]], "targets"),
    "target_length": as_int([1], "target_length"),
    "input_states_1": state[0],
    "input_states_2": state[1],
}
names = ["outputs", "output_states_1", "output_states_2"]
for label, session in (("cuda", asr._decoder_joint), ("cpu default threads", cpu_dec), ("cpu 1 thread", cpu_dec1)):
    for _ in range(20):
        session.run(names, feeds)
    times = []
    for _ in range(300):
        t0 = time.perf_counter()
        out = session.run(names, feeds)
        times.append((time.perf_counter() - t0) * 1e6)
    print(f"decoder {label}: p50 {np.percentile(times, 50):.0f} us, p95 {np.percentile(times, 95):.0f} us", flush=True)
ref = asr._decoder_joint.run(names, feeds)[0]
for label, session in (("cpu default threads", cpu_dec), ("cpu 1 thread", cpu_dec1)):
    got = session.run(names, feeds)[0]
    print(f"{label} vs cuda: max abs diff {np.abs(got - ref).max():.2e}, argmax equal {got.argmax() == ref.argmax()}")
print("decoder inputs", {k: (v.dtype, v.shape) for k, v in feeds.items()})
