# The Windows app in Rust, measured against Python

On 2026-11-05 the Windows app was rewritten from Python to Rust. This page compares the rewrite with the last Python version, commit 5d4e136. The Mac app stayed in Swift.

Startup fell 24% and idle memory 8%. GPU memory and transcription latency did not move, because the model dominates both. The overlay, the meter and the Settings window gained the most, and the app no longer needs Python.

## Results

Medians of 5 processes per side on CUDA, alternated in one session on the same machine.

| Metric | Python | Rust | Change |
|---|---:|---:|---:|
| Ready to dictate | 2.60 s | 1.97 s | -24% |
| Idle working set | 1276 MB | 1176 MB | -8% |
| Idle private working set | 1118 MB | 1034 MB | -7% |
| Idle commit | 7193 MB | 6886 MB | -4% |
| Idle GPU memory | 5630 MB | 5640 MB | none |
| Threads | 17 | 15 | -2 |
| CPU over the whole run | 11.7 s | 10.3 s | -12% |
| Dictation latency, every clip | 71.3 ms | 70.1 ms | within noise |
| 110 s clip, new length | 473 ms | 451 ms | within range |
| Overlay render, p50, four cases | 1.8 to 4.1 ms | 0.4 to 1.2 ms | 3 to 5 times faster |
| Level meter, p50 | 45.6 µs | 2.3 µs | 20 times faster |
| Settings window appears | 0.35 s | 0.16 s | -54% |
| Settings working set | 55.6 MB | 18.2 MB | -67% |
| Settings private working set | 30.3 MB | 2.0 MB | -93% |
| Settings threads | 7 | 4 | -3 |
| Disk, without the model | 2.0 GB `.venv` | 1.3 GB | -35% |

All 83 transcripts on CUDA match Python's. The executable is 5.9 MB, and the disk figure counts it with the 13 runtime DLLs the app loads.

The real tray app, one round per side with the overlay and sounds off, idled at 1152 MB in Python and 1104 MB in Rust, -4%, and was ready in 2.5 s and 1.9 s. That round has no dictations in it, so it is the cleaner look at the app itself and the weaker sample: five rounds were planned and did not run.

## What did not improve or differs

- **CPU provider.** 3 of 35 transcripts differ from Python's, for example "às três" against "às 3". Python used NumPy for the audio features on the CPU and Rust uses the ONNX graph it uses on CUDA. This run was cut short: 3 processes of Python and 2 of Rust. Idle memory and latency stayed level.
- **Overlay pixels.** Frames match to within 2 levels per channel, not exactly.
- **Audio input.** cpal and WASAPI replaced PortAudio and MME.
- **Settings.** The window is native Win32 in place of Tk, and is system DPI aware.
- **HTTP.** `ureq` replaced httpx2. The server for Mac answers with compact JSON and drops the connection when transcription fails, as the Python handler did.

## Not measured or not verified

- Hook latency against Python. The Rust E2E run took about 100 ms from key to Ctrl+V, but there is no Python number from the same session, so this says nothing about the hotkey's robustness.
- A dictation with a real voice. The default microphone returned digital silence on this machine in both Python and Rust; cpal did capture a nonzero signal from another device.
- A real GLM or Claude reply. The E2E run and the benchmark use local stand-ins.
- A real Mac against the Rust server. `parakeet-check server` posts the fixtures from a local client.

## Reproduce

Check out 5d4e136 next to this tree, with its own virtual environment; the harness needs that environment's NumPy to export the audio both sides receive. From this tree, with the release build in `target\release`, run the harness with that environment's Python:

```powershell
python scripts/bench_resources.py model --provider cuda --side python=<checkout>/src --side rust=target/release --model-dir models/parakeet-ultra --runs 5 --warmup 1 --reps 5 --out <folder>
python scripts/bench_resources.py overlay --side python=<checkout>/src --side rust=target/release --runs 5 --warmup 1 --repeats 3 --out <folder>
python scripts/bench_resources.py settings --side python=<checkout>/src --side rust=target/release --runs 5 --warmup 1 --out <folder>
```

The Rust child is `parakeet-bench`. It runs the same work on the same audio, and the harness reads its counters from outside, at the same points as the Python child's. The data behind this page is in [`data/rust/after`](data/rust/after); [`data/rust/before`](data/rust/before) is the Python baseline taken before the rewrite started.
