"""Exploratory: score the CPU-provider transcripts that differ between sides against the fixtures' text.

The tiled clips' reference repeats the English and Portuguese texts once per started unit, so it ends
past the clip's cut. That costs every side the same deletions; compare the sides, not the absolute rates.
usage: cpu_transcripts.py SUMMARY_JSON
"""

import json
import math
import sys
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "scripts"))
import bench_resources as b  # noqa: E402
from check_model import word_error_rate, words  # noqa: E402


def seconds(name: str) -> float:
    with wave.open(str(b.FIXTURES / f"{name}.wav")) as f:
        return f.getnframes() / f.getframerate()


en, pt = ((b.FIXTURES / f"{name}.txt").read_text(encoding="utf-8") for name in ("en", "pt"))
unit = seconds("en") + seconds("pt") + 2 * b.GAP_S
summary = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))["model_transcripts"]
print(f"{summary['calls']} call positions, {summary['calls_differing_between_sides']} differ between sides")
for d in summary["differences"]:
    clip = list(b.CLIPS)[d["call"] % len(b.CLIPS)]
    reference = words(" ".join([en, pt] * math.ceil(b.CLIPS[clip] / unit)))
    print(f"{d['set']} call {d['call']} ({clip}, {b.CLIPS[clip]} s)")
    for side, texts in d.items():
        if side not in ("set", "call"):
            for text in texts:
                print(f"  {side}: WER {word_error_rate(reference, words(text)):.4f}, {len(words(text))} words: {text}")
