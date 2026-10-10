"""Transcribe the TTS fixtures and report accuracy, latency, and the provider that actually ran."""

import argparse
import re
import sys
import time
import unicodedata
from pathlib import Path

import onnx_asr

ROOT = Path(__file__).resolve().parent.parent
MODEL_DIR = ROOT / "models" / "parakeet-tdt-0.6b-v3"
FIXTURES = ROOT / "tests" / "fixtures"
PROVIDERS = {
    "cpu": ["CPUExecutionProvider"],
    "cuda": ["CUDAExecutionProvider", "CPUExecutionProvider"],
}


def words(text: str) -> list[str]:
    text = unicodedata.normalize("NFC", text.lower())
    return re.findall(r"\w+", text)


def word_error_rate(ref: list[str], hyp: list[str]) -> float:
    prev = list(range(len(hyp) + 1))
    for i, r in enumerate(ref, 1):
        cur = [i] + [0] * len(hyp)
        for j, h in enumerate(hyp, 1):
            cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (r != h))
        prev = cur
    return prev[-1] / max(len(ref), 1)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--provider", choices=PROVIDERS, default="cpu")
    parser.add_argument("--quantization", choices=["int8", "none"], default="int8")
    parser.add_argument("--runs", type=int, default=3)
    args = parser.parse_args()

    if args.provider == "cuda":
        import onnxruntime

        onnxruntime.preload_dlls()

    t0 = time.perf_counter()
    model = onnx_asr.load_model(
        "nemo-parakeet-tdt-0.6b-v3",
        MODEL_DIR,
        quantization=None if args.quantization == "none" else args.quantization,
        providers=PROVIDERS[args.provider],
    )
    print(f"load: {time.perf_counter() - t0:.2f}s")
    print(f"encoder providers: {model.asr._encoder.get_providers()}")

    ok = True
    for wav in sorted(FIXTURES.glob("*.wav")):
        expected = wav.with_suffix(".txt").read_text(encoding="utf-8").strip()
        model.recognize(str(wav))
        times = []
        for _ in range(args.runs):
            t0 = time.perf_counter()
            text = model.recognize(str(wav))
            times.append(time.perf_counter() - t0)
        wer = word_error_rate(words(expected), words(text))
        ok &= wer <= 0.15
        print(f"[{wav.stem}] best {min(times) * 1000:.0f} ms  WER {wer:.2f}\n  want: {expected}\n  got:  {text}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
