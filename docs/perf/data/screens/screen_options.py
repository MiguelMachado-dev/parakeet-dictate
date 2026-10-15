"""Run probe_options.py for each config, interleaved, and print a compact comparison (exploratory screen)."""

import json
import statistics
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).parent
PY = r"C:\Users\migue\.t3\worktrees\parakeet-dictate\t3-073be6d8\.venv\Scripts\python.exe"
configs = sys.argv[2:]
rounds = int(sys.argv[1])
out = HERE / "screen.jsonl"
results = {c: [] for c in configs}
for r in range(rounds):
    for c in configs:
        proc = subprocess.run([PY, str(HERE / "probe_options.py"), c], capture_output=True, text=True, encoding="utf-8")
        line = [l for l in proc.stdout.splitlines() if l.startswith("{")]
        if proc.returncode or not line:
            print(c, "FAILED", proc.stderr[-2000:], flush=True)
            continue
        data = json.loads(line[-1])
        results[c].append(data)
        with out.open("a", encoding="utf-8") as f:
            f.write(line[-1] + "\n")
        print(f"round {r} {c} done", flush=True)


def med(values):
    return statistics.median(values) if values else float("nan")


texts = {}
for c, runs in results.items():
    if not runs:
        continue
    snap = lambda run, label: next(s for s in run["snaps"] if s["label"] == label)
    row = {
        "load_s": med([r["load_s"] for r in runs]),
        "warm_s": med([r["warm_s"] for r in runs]),
        "thr_loaded": med([snap(r, "loaded")["threads"] for r in runs]),
        "gpu_loaded": med([snap(r, "loaded")["gpu"] for r in runs]),
        "gpu_warm": med([snap(r, "warm")["gpu"] for r in runs]),
        "gpu_after": med([snap(r, "after")["gpu"] for r in runs]),
        "commit_after": med([snap(r, "after")["commit"] for r in runs]),
        "pws_after": med([snap(r, "after")["pws"] for r in runs]),
        "cpu_load_warm": med([snap(r, "warm")["cpu_s"] - snap(r, "imported")["cpu_s"] for r in runs]),
        "cpu_clips": med([snap(r, "after")["cpu_s"] - snap(r, "warm")["cpu_s"] for r in runs]),
        "cpu_idle5": med([snap(r, "idle5")["cpu_s"] - snap(r, "after")["cpu_s"] for r in runs]),
        "en_fresh_ms": med([row["ms"] for r in runs for row in r["rows"] if row["clip"] == "en"]),
        "en_same_ms": med([ms for r in runs for ms in r["same_en_ms"][1:]]),
        "xlong_ms": med([row["ms"] for r in runs for row in r["rows"] if row["clip"] == "xlong"]),
        "mid_ms": med([row["ms"] for r in runs for row in r["rows"] if row["clip"] == "mid"]),
    }
    print(c, json.dumps({k: round(v, 2) for k, v in row.items()}))
    texts[c] = {tuple(row["text"] for row in r["rows"]) for r in runs}
base = next(iter(texts.values()))
for c, t in texts.items():
    print(c, "distinct transcript sets:", len(t), "same as first config:", t == base)
