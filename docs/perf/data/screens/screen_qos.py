"""Run probe_qos.py per config, interleaved; print latency medians and how often calls ran on P-cores (0-7)."""

import json
import statistics
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).parent
PY = r"C:\Users\migue\.t3\worktrees\parakeet-dictate\t3-073be6d8\.venv\Scripts\python.exe"
rounds, configs = int(sys.argv[1]), sys.argv[2:]
results = {c: [] for c in configs}
for r in range(rounds):
    for c in configs:
        proc = subprocess.run([PY, str(HERE / "probe_qos.py"), c], capture_output=True, text=True, encoding="utf-8")
        line = [l for l in proc.stdout.splitlines() if l.startswith("{")]
        if proc.returncode or not line:
            print(c, "FAILED", proc.stderr[-1500:], flush=True)
            continue
        data = json.loads(line[-1])
        results[c].append(data)
        with (HERE / "screen_qos.jsonl").open("a", encoding="utf-8") as f:
            f.write(line[-1] + "\n")
        fresh = [x["ms"] for x in data["rows"] if x["kind"] == "fresh"]
        same = [x["ms"] for x in data["rows"] if x["kind"] == "same"]
        p = sum(all(core < 8 for core in x["cores"]) for x in data["rows"]) / len(data["rows"])
        print(f"round {r} {c}: fresh p50 {statistics.median(fresh):.1f} same p50 {statistics.median(same):.1f} P-core share {p:.2f}", flush=True)
for c, runs in results.items():
    rows = [x for run in runs for x in run["rows"]]
    if not rows:
        continue
    for kind in ("fresh", "same"):
        ms = [x["ms"] for x in rows if x["kind"] == kind]
        on_p = [x["ms"] for x in rows if x["kind"] == kind and all(core < 8 for core in x["cores"])]
        on_e = [x["ms"] for x in rows if x["kind"] == kind and all(core >= 8 for core in x["cores"])]
        print(f"{c} {kind}: n {len(ms)} p50 {statistics.median(ms):.1f} min {min(ms):.1f} max {max(ms):.1f} | "
              f"P-only n {len(on_p)} p50 {statistics.median(on_p) if on_p else float('nan'):.1f} | "
              f"E-only n {len(on_e)} p50 {statistics.median(on_e) if on_e else float('nan'):.1f}")
