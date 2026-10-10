import argparse
import json
import statistics
import subprocess
import sys
import time
import wave
from pathlib import Path

import numpy as np

from parakeet_dictate.config import Config
from parakeet_dictate.recorder import Audio
from parakeet_dictate.transcriber import WARMUP_SECONDS, load_transcriber, resample

FIXTURES = Path(__file__).resolve().parent.parent / "tests" / "fixtures"
APP_DEFAULT = "unset"
MODES = (APP_DEFAULT, "EXHAUSTIVE", "HEURISTIC", "DEFAULT")
UNSEEN_SECONDS = (1.3, 2.1, 2.7, 3.3, 4.6, 5.9, 6.4, 7.9, 9.2, 11.3, 14.6, 17.9)
MIC_RATE = 44_100
IDLE_GAP_S = 8.0
IDLE_RUNS = (
    ("seen", UNSEEN_SECONDS[5]), ("unseen", 3.9),
    ("seen", UNSEEN_SECONDS[8]), ("unseen", 7.1),
    ("seen", UNSEEN_SECONDS[2]), ("unseen", 12.4),
)
assert not {s for label, s in IDLE_RUNS if label == "unseen"} & set(UNSEEN_SECONDS)


def read_fixture(name: str) -> np.ndarray:
    with wave.open(str(FIXTURES / f"{name}.wav")) as f:
        assert f.getframerate() == 16_000 and f.getsampwidth() == 2 and f.getnchannels() == 1
        return np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768


def timed_ms(transcriber, audio: Audio) -> tuple[float, str]:
    t0 = time.perf_counter()
    text = transcriber.transcribe(audio)
    return (time.perf_counter() - t0) * 1000, text


def measure(mode: str) -> dict:
    t0 = time.perf_counter()
    algo_search = None if mode == APP_DEFAULT else mode
    transcriber = load_transcriber(Config().model_dir, "cuda", cudnn_conv_algo_search=algo_search)
    load_s = time.perf_counter() - t0
    assert transcriber.provider == "CUDAExecutionProvider", transcriber.provider

    t0 = time.perf_counter()
    transcriber.warm_up(MIC_RATE)
    warmup_ms = (time.perf_counter() - t0) * 1000

    speech = resample(np.concatenate([read_fixture("en"), read_fixture("pt")]), 16_000, MIC_RATE)
    speech = np.tile(speech, int(np.ceil(max(UNSEEN_SECONDS) * MIC_RATE / len(speech))))
    rows = []
    for seconds in UNSEEN_SECONDS:
        audio = Audio(speech[: int(seconds * MIC_RATE)], MIC_RATE)
        first_ms, text = timed_ms(transcriber, audio)
        steady_ms = min(timed_ms(transcriber, audio)[0] for _ in range(3))
        rows.append({"seconds": seconds, "first_ms": first_ms, "steady_ms": steady_ms, "words": len(text.split())})

    idle = {}
    for label, seconds in IDLE_RUNS:
        time.sleep(IDLE_GAP_S)
        idle.setdefault(label, []).append(timed_ms(transcriber, Audio(speech[: int(seconds * MIC_RATE)], MIC_RATE))[0])
    return {"mode": mode, "load_s": load_s, "warmup_ms": warmup_ms, "rows": rows, "idle": idle}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=MODES)
    args = parser.parse_args()
    if args.mode:
        print(json.dumps(measure(args.mode)))
        return 0

    results = [
        json.loads(
            subprocess.run(
                [sys.executable, __file__, "--mode", mode], capture_output=True, text=True, check=True
            ).stdout.strip().splitlines()[-1]
        )
        for mode in MODES
    ]

    print(f"warm-up {WARMUP_SECONDS} s @ {MIC_RATE} Hz, then each length once (first) and best of 3 more (steady), ms")
    print(f"{'audio':>6} {'words':>5}  " + "  ".join(f"{r['mode']:>17}" for r in results))
    for i, seconds in enumerate(UNSEEN_SECONDS):
        cells = "  ".join(f"{r['rows'][i]['first_ms']:>8.0f} / {r['rows'][i]['steady_ms']:<6.0f}" for r in results)
        print(f"{seconds:>5.1f}s {results[0]['rows'][i]['words']:>5}  {cells}")
    medians = "  ".join(
        f"{statistics.median(row['first_ms'] for row in r['rows']):>8.0f} / "
        f"{statistics.median(row['steady_ms'] for row in r['rows']):<6.0f}"
        for r in results
    )
    print(f"{'median':>12}  {medians}")
    for r in results:
        after_idle = ", ".join(f"{label} {[round(ms) for ms in times]}" for label, times in r["idle"].items())
        print(
            f"{r['mode']}: load {r['load_s']:.1f} s, warm-up {r['warmup_ms']:.0f} ms; "
            f"after {IDLE_GAP_S:.0f} s idle: {after_idle}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
