# Resource and latency measurements

This folder holds before-and-after measurements of the Windows app's memory, GPU memory, threads, CPU time, startup and latency, the changes they justified, and the raw data behind every number. The baseline is da3c189. `scripts/bench_resources.py` produced the data in `data/baseline` and `data/stages` and printed every table below. [`tables.md`](tables.md) holds the complete tables, with ranges, p95 and sample counts, under the commands that print them. `data/screens` holds the exploratory screens behind each decision, and `decisions.tsv` the decision log.

## Summary

On CUDA, the path the installed app runs, every memory, thread and CPU-time measure fell or held within its range, no latency median rose, and every transcript matched. The model process is ready 2.6 s sooner and spends 2.9 s of CPU time loading and warming up instead of 67.5 s. In the 5 s after a transcription it burns 0.05 s of CPU instead of 4.47 s. It holds 17 threads instead of 226, 410 MB less GPU memory and 1088 MB less commit, and a dictation reaches the paste 68 ms after the key is released instead of 113 ms at the median, 115 ms instead of 174 ms at p95. The Settings window opens in 0.33 s instead of 0.57 s, with 33 MB of commit instead of 716 MB. The overlay renders the same pixels in the same time, with 4 threads instead of 23.

On the CPU provider, which the app falls back to without CUDA, commits 1 to 5 cut threads from 194 to 62 and commit by 632 MB, and commit 6 cut the CPU burned in the 5 s after a transcription from 11.12 s to 0.03 s and a dictation's median latency from 1112 ms to 460 ms, with every transcript matching. On that path, commit 4 changes the rounding of the mel features, which changed the words of 2 long transcripts out of 51 at the same error rate.

The model process on CUDA, da3c189 against the final tree, 5 processes each, alternated in one session. Cells are medians; latencies pool every call of a side.

| metric | before: base, n=5 | after: final, n=5 | change | change % |
|---|---:|---:|---:|---:|
| ready_since_start_s | 5.91 [5.86-6.37] p95 6.28, n 5 | 3.30 [3.20-4.24] p95 4.06, n 5 | -2.61 | -44.2% |
| loaded.gpu_dedicated_mb | 4109.55 [4101.54-4113.55] p95 4113.55, n 5 | 3637.54 [3629.54-3645.54] p95 3645.54, n 5 | -472.00 | -11.5% |
| idle_30s.working_set_mb | 1393.79 [1391.05-1397.40] p95 1397.02, n 5 | 1330.89 [1326.06-1332.46] p95 1332.14, n 5 | -62.91 | -4.5% |
| idle_30s.private_working_set_mb | 1230.83 [1228.08-1234.44] p95 1234.06, n 5 | 1168.12 [1163.29-1169.69] p95 1169.38, n 5 | -62.71 | -5.1% |
| idle_30s.commit_mb | 8483.34 [8475.05-8492.57] p95 8492.28, n 5 | 7395.34 [7282.04-7412.50] p95 7410.86, n 5 | -1088.00 | -12.8% |
| idle_30s.threads | 226.00 [226.00-226.00] p95 226.00, n 5 | 17.00 [17.00-17.00] p95 17.00, n 5 | -209.00 | -92.5% |
| idle_30s.gpu_dedicated_mb | 6037.56 [6031.56-6041.56] p95 6041.56, n 5 | 5627.56 [5515.56-5643.56] p95 5641.96, n 5 | -410.00 | -6.8% |
| peak_working_set_mb | 1421.89 [1419.38-1425.51] p95 1425.13, n 5 | 1358.88 [1354.44-1360.50] p95 1360.19, n 5 | -63.01 | -4.4% |
| peak_commit_mb | 8677.58 [8669.30-8686.82] p95 8686.53, n 5 | 7589.56 [7476.24-7606.71] p95 7605.08, n 5 | -1088.02 | -12.5% |
| cpu_load_and_warm_s | 67.52 [66.31-67.98] p95 67.89, n 5 | 2.94 [2.73-3.66] p95 3.53, n 5 | -64.58 | -95.6% |
| idle_cpu_0_5s | 4.47 [2.59-4.78] p95 4.72, n 5 | 0.05 [0.03-0.06] p95 0.06, n 5 | -4.42 | -99.0% |
| new_length.en.ms | 114.29 [59.29-163.75] p95 143.59, n 20 | 63.56 [58.64-93.23] p95 78.09, n 20 | -50.73 | -44.4% |
| new_length.xlong.ms | 422.34 [343.94-477.09] p95 469.12, n 20 | 372.85 [354.93-475.39] p95 457.50, n 20 | -49.49 | -11.7% |
| same_length.en.ms | 35.69 [28.49-64.34] p95 62.99, n 20 | 32.69 [29.97-39.13] p95 38.57, n 20 | -3.00 | -8.4% |
| dictation.all.ms | 113.42 [61.00-193.52] p95 173.93, n 100 | 68.09 [51.97-126.58] p95 114.89, n 100 | -45.33 | -40.0% |
| dictation_polish_mock.all.ms | 113.61 [79.78-178.51] p95 149.88, n 30 | 63.02 [50.77-79.36] p95 69.86, n 30 | -50.59 | -44.5% |
| wer.en | 0.00 [0.00-0.00] p95 0.00, n 25 | 0.00 [0.00-0.00] p95 0.00, n 25 | +0.00 | - |
| wer.pt | 0.08 [0.08-0.08] p95 0.08, n 25 | 0.08 [0.08-0.08] p95 0.08, n 25 | +0.00 | +0.0% |

Failed runs: before 0, after 0. Sources: before e2ba329436730af9, after 4fc7953e9760a865.
Transcripts: 83 call positions compared, 0 differ.

The Settings window, `pythonw -m parakeet_dictate --settings`, 5 processes each:

| metric | before: base, n=5 | after: final, n=5 | change | change % |
|---|---:|---:|---:|---:|
| window_s | 0.57 [0.55-0.68] p95 0.66, n 5 | 0.33 [0.32-0.35] p95 0.35, n 5 | -0.24 | -41.6% |
| working_set_mb | 83.73 [83.57-83.80] p95 83.80, n 5 | 54.63 [54.42-54.78] p95 54.77, n 5 | -29.10 | -34.8% |
| private_working_set_mb | 48.50 [48.31-48.54] p95 48.54, n 5 | 30.07 [29.86-30.23] p95 30.23, n 5 | -18.43 | -38.0% |
| commit_mb | 715.94 [715.70-716.01] p95 716.00, n 5 | 33.32 [32.99-33.51] p95 33.49, n 5 | -682.62 | -95.3% |
| threads | 28.00 [28.00-28.00] p95 28.00, n 5 | 7.00 [7.00-8.00] p95 7.80, n 5 | -21.00 | -75.0% |
| modules | 107.00 [107.00-107.00] p95 107.00, n 5 | 71.00 [71.00-71.00] p95 71.00, n 5 | -36.00 | -33.6% |

Failed runs: before 0, after 0. Sources: before e2ba329436730af9, after 4fc7953e9760a865.

The overlay, a scripted 12.5 s of frames per style and scale, 5 processes each:

| metric | before: base, n=5 | after: final, n=5 | change | change % |
|---|---:|---:|---:|---:|
| bars@1.p50_ms | 1.79 [1.76-1.81] p95 1.81, n 5 | 1.78 [1.75-1.83] p95 1.82, n 5 | -0.01 | -0.6% |
| bars@1.p95_ms | 2.07 [2.01-2.19] p95 2.17, n 5 | 2.05 [2.03-2.23] p95 2.20, n 5 | -0.03 | -1.2% |
| bars@1.5.p50_ms | 3.92 [3.84-3.94] p95 3.93, n 5 | 3.92 [3.76-4.23] p95 4.18, n 5 | -0.01 | -0.2% |
| bars@1.5.p95_ms | 4.40 [4.35-4.44] p95 4.43, n 5 | 4.42 [4.18-6.95] p95 6.44, n 5 | +0.02 | +0.4% |
| fluid@1.p50_ms | 1.89 [1.85-2.06] p95 2.03, n 5 | 1.86 [1.82-2.01] p95 1.99, n 5 | -0.03 | -1.5% |
| fluid@1.p95_ms | 2.46 [2.41-2.56] p95 2.54, n 5 | 2.39 [2.34-2.79] p95 2.73, n 5 | -0.07 | -2.7% |
| fluid@1.5.p50_ms | 3.82 [3.67-5.01] p95 4.78, n 5 | 3.80 [3.47-3.85] p95 3.84, n 5 | -0.02 | -0.5% |
| fluid@1.5.p95_ms | 4.89 [4.52-7.70] p95 7.21, n 5 | 4.70 [4.48-4.75] p95 4.74, n 5 | -0.19 | -3.9% |
| meter.p50_us | 41.75 [41.30-42.20] p95 42.17, n 5 | 42.00 [33.45-42.20] p95 42.17, n 5 | +0.25 | +0.6% |
| threads | 23.00 [23.00-23.00] p95 23.00, n 5 | 4.00 [4.00-4.00] p95 4.00, n 5 | -19.00 | -82.6% |

Failed runs: before 0, after 0. Sources: before e2ba329436730af9, after 4fc7953e9760a865.
Frames: every run of both sides rendered the same pixels.

The CPU provider, which the app falls back to without CUDA, 3 processes per side with 3 rounds of clips. Commits 1 to 5, da3c189 against 35fb1f7, with the 2 differing transcripts that the Transcripts section explains:

| metric | before: base, n=3 | after: final, n=3 | change | change % |
|---|---:|---:|---:|---:|
| ready_since_start_s | 11.84 [6.44-12.28] p95 12.24, n 3 | 4.73 [4.47-4.77] p95 4.77, n 3 | -7.11 | -60.0% |
| idle_30s.working_set_mb | 2052.16 [2051.79-2064.04] p95 2062.85, n 3 | 2017.36 [2016.70-2028.65] p95 2027.52, n 3 | -34.80 | -1.7% |
| idle_30s.commit_mb | 3642.49 [3642.22-3642.51] p95 3642.51, n 3 | 3010.60 [3010.59-3011.25] p95 3011.19, n 3 | -631.89 | -17.3% |
| idle_30s.threads | 194.00 [192.00-195.00] p95 194.90, n 3 | 62.00 [59.00-62.00] p95 62.00, n 3 | -132.00 | -68.0% |
| cpu_total_s | 2279.81 [1470.30-2723.00] p95 2678.68, n 3 | 1362.34 [1352.64-1485.52] p95 1473.20, n 3 | -917.47 | -40.2% |
| cpu_load_and_warm_s | 106.59 [74.77-108.52] p95 108.32, n 3 | 34.12 [28.56-36.81] p95 36.54, n 3 | -72.47 | -68.0% |
| idle_cpu_0_5s | 11.39 [11.05-11.41] p95 11.40, n 3 | 11.42 [10.31-12.28] p95 12.20, n 3 | +0.03 | +0.3% |
| new_length.short.ms | 1695.88 [494.79-2831.57] p95 2822.61, n 6 | 783.12 [665.54-962.06] p95 932.70, n 6 | -912.77 | -53.8% |
| new_length.en.ms | 2086.00 [715.92-3282.08] p95 3249.34, n 6 | 867.07 [652.02-1287.49] p95 1225.40, n 6 | -1218.93 | -58.4% |
| new_length.long.ms | 5587.56 [2114.31-8859.86] p95 8739.95, n 6 | 2798.38 [2322.53-4605.71] p95 4584.65, n 6 | -2789.18 | -49.9% |
| new_length.xlong.ms | 17269.56 [4304.50-17776.20] p95 17740.71, n 6 | 5554.40 [4587.83-8263.03] p95 7921.54, n 6 | -11715.16 | -67.8% |
| dictation.all.ms | 1949.93 [1047.77-3986.16] p95 3928.11, n 36 | 1116.85 [440.93-3022.82] p95 2933.60, n 36 | -833.08 | -42.7% |
| pcore_share.dictation | 0.00 [0.00-0.00] p95 0.00, n 3 | 1.00 [0.92-1.00] p95 1.00, n 3 | +1.00 | - |

Failed runs: before 0, after 0. Sources: before e2ba329436730af9, after 4fc7953e9760a865.
Transcripts: 51 call positions compared, 2 differ: [('new_length', 10), ('new_length', 17)]

Commit 6, 35fb1f7 against the same tree with `session.force_spinning_stop`, the option 38e4a05 sets:

| metric | before: final, n=3 | after: stopspin, n=3 | change | change % |
|---|---:|---:|---:|---:|
| ready_since_start_s | 5.07 [5.00-5.59] p95 5.54, n 3 | 3.92 [3.78-4.22] p95 4.19, n 3 | -1.15 | -22.7% |
| idle_30s.working_set_mb | 2016.82 [2016.77-2017.77] p95 2017.68, n 3 | 2016.67 [2016.43-2017.66] p95 2017.56, n 3 | -0.15 | -0.0% |
| idle_30s.commit_mb | 3010.95 [3010.53-3011.60] p95 3011.54, n 3 | 3010.69 [3010.37-3011.87] p95 3011.75, n 3 | -0.27 | -0.0% |
| idle_30s.threads | 62.00 [61.00-62.00] p95 62.00, n 3 | 62.00 [62.00-62.00] p95 62.00, n 3 | +0.00 | +0.0% |
| cpu_total_s | 1448.50 [1446.06-1589.84] p95 1575.71, n 3 | 919.77 [897.75-967.88] p95 963.06, n 3 | -528.73 | -36.5% |
| cpu_load_and_warm_s | 44.59 [44.53-45.73] p95 45.62, n 3 | 22.84 [19.92-26.48] p95 26.12, n 3 | -21.75 | -48.8% |
| idle_cpu_0_5s | 11.12 [10.23-11.53] p95 11.49, n 3 | 0.03 [0.03-0.05] p95 0.05, n 3 | -11.09 | -99.7% |
| new_length.short.ms | 870.55 [859.90-1107.31] p95 1104.34, n 6 | 264.97 [131.05-468.55] p95 434.43, n 6 | -605.58 | -69.6% |
| new_length.en.ms | 917.64 [450.70-1443.17] p95 1391.96, n 6 | 469.90 [233.66-827.05] p95 809.07, n 6 | -447.73 | -48.8% |
| new_length.long.ms | 2801.86 [2409.71-3364.63] p95 3344.15, n 6 | 2779.28 [1500.27-4419.37] p95 4035.68, n 6 | -22.58 | -0.8% |
| new_length.xlong.ms | 5750.61 [4955.38-7659.02] p95 7210.53, n 6 | 5990.22 [4926.81-7707.07] p95 7523.64, n 6 | +239.61 | +4.2% |
| dictation.all.ms | 1111.91 [669.33-2272.44] p95 1879.93, n 36 | 460.27 [114.56-1733.28] p95 1425.28, n 36 | -651.63 | -58.6% |
| pcore_share.dictation | 1.00 [1.00-1.00] p95 1.00, n 3 | 0.92 [0.83-1.00] p95 0.99, n 3 | -0.08 | -8.3% |

Failed runs: before 0, after 0. Sources: before 4fc7953e9760a865, after 983b9db631d9d62f.
Transcripts: 51 call positions compared, 0 differ.

A session of da3c189 against 38e4a05 itself was stopped after 2 measured processes per side, when this work paused. Running it is the first item of [`next-steps.md`](next-steps.md).

## What changed

Six commits. The stages sessions below measured the tree after each one.

1. **fcb4a74 Keep transcriptions off efficiency cores.** `Transcriber.transcribe` tags its thread high QoS with `SetThreadInformation(ThreadPowerThrottling)`, which Microsoft documents as the way to opt a thread into the High QoS level. Untagged, Windows ran the transcribing thread on an efficiency core (logical CPUs 8 to 19 on this i7-14700KF) in some sessions. In the `probe_qos.py` screen, it ran there in every untagged call, 54 per configuration, and the English clip at new lengths took 168.9 ms against 39.9 ms tagged with the default pools, and 70 ms against 38.6 ms with one intra-op thread (`screens/qos.log`). The tag covers the dictation worker, the Mac server's request threads and the warm-up.
2. **4fcc2d0 Run the CUDA sessions without per-core thread pools.** Every ONNX Runtime session started an intra-op pool of one thread per core (20 here) that spins after each run. On CUDA, the CPU side of the graphs is shape arithmetic on a few integers, in 482 of the encoder's nodes such as `Unsqueeze`, `Gather`, `Concat` and `Slice`. The decoder runs entirely on the GPU. One intra-op thread per session removes the pools. The CPU provider keeps its pools, since there the encoder needs them.
3. **f66a11a Open a resampler session only for sample rates the app receives.** onnx_asr opens a resampler session for each of seven input rates when the model loads, about 70 MB of GPU memory each on CUDA and one thread pool each on the CPU provider. The microphone records at one rate, and the warm-up runs at that rate. `LazyResampler` opens each session on first use. Tests check that it opens nothing until a rate arrives and that its output equals onnx_asr's `Resampler` bit for bit at 44.1 and 48 kHz.
4. **3998e3a Keep numpy's OpenBLAS to one thread.** Importing numpy started an OpenBLAS thread per core and committed a buffer for each, 611 MB of commit and 19 threads in the stages session. The package sets `OPENBLAS_NUM_THREADS=1`, unless the environment sets it, before anything imports numpy. The app's matrix products are the fluid overlay style's and, on the CPU provider, the mel filterbank in onnx_asr's numpy preprocessor. The overlay renders the same pixels in the same time, and the preprocessor takes the same time on one thread, 3.2 ms, 27 to 28 ms and 58 to 62 ms for 6 s, 50 s and 110 s of audio (`screens/mel.log`). On the CPU provider, the thread count changes the features' rounding, as the Transcripts section below details.
5. **b50b4aa Open Settings without loading the model and audio stack.** `python -m parakeet_dictate --settings` imported the app module, and with it ONNX Runtime, PortAudio, pystray and the transcriber, before it reached the Settings window. `__main__` now dispatches that exact command line first, and the named mutex and events that Settings shares with the tray app moved to `instance.py`.
6. **38e4a05 Stop the CPU provider's thread pools spinning after each run.** On the CPU provider, every session keeps its default pool of a thread per core, which the encoder needs, and each pool spins after every run. It spins through the other sessions' runs, such as the decoder's run per token, and for seconds after a transcription ends. `session.force_spinning_stop`, which ONNX Runtime documents as a way to "decrease CPU usage between infrequent requests", stops a session's pool when its last run returns and restarts it on the next run. The CUDA sessions run with one intra-op thread and have no pools to stop.

## Stages

### CUDA

One session alternated the baseline and the tree after each commit: base is da3c189, c1 to c3 are fcb4a74, 4fcc2d0 and f66a11a, and final is 35fb1f7, whose `src` equals b50b4aa's. Between c3 and final, the model process changes only by commit 4, since commit 5 moves the app's mutex and event code without changing it and otherwise touches the Settings path. Commit 6 changes only the CPU provider's branch, so on CUDA the final tree runs the same code as 38e4a05. Medians of 5 processes per side:

| metric | base, n=5 | c1, n=5 | c2, n=5 | c3, n=5 | final, n=5 |
|---|---:|---:|---:|---:|---:|
| warm_up_s | 2.70 | 2.71 | 0.48 | 0.49 | 0.49 |
| ready_since_start_s | 5.91 | 5.98 | 3.33 | 3.29 | 3.30 |
| imported.commit_mb | 661.96 | 661.83 | 661.86 | 662.09 | 51.45 |
| loaded.gpu_dedicated_mb | 4109.55 | 4113.55 | 4109.55 | 3633.54 | 3637.54 |
| idle_30s.working_set_mb | 1393.79 | 1393.79 | 1336.98 | 1333.50 | 1330.89 |
| idle_30s.commit_mb | 8483.34 | 8483.68 | 8415.84 | 8012.95 | 7395.34 |
| idle_30s.threads | 226.00 | 226.00 | 36.00 | 36.00 | 17.00 |
| idle_30s.gpu_dedicated_mb | 6037.56 | 6041.56 | 6029.56 | 5631.56 | 5627.56 |
| cpu_load_and_warm_s | 67.52 | 67.89 | 2.95 | 2.78 | 2.94 |
| idle_cpu_0_5s | 4.47 | 4.27 | 0.03 | 0.05 | 0.05 |
| new_length.en.ms | 114.29 | 91.74 | 67.96 | 63.68 | 63.56 |
| new_length.xlong.ms | 422.34 | 399.73 | 385.34 | 378.58 | 372.85 |
| dictation.all.ms | 113.42 | 107.04 | 64.08 | 64.16 | 68.09 |
| pcore_share.new_length | 0.53 | 0.83 | 1.00 | 1.00 | 1.00 |
| pcore_share.dictation | 0.00 | 0.05 | 1.00 | 1.00 | 1.00 |

- **c1, the tag.** Memory, threads and CPU time stay where they were. The main thread's calls moved toward performance cores (`pcore_share.new_length` 0.53 to 0.83) and new-length medians fell, the English clip from 114 ms to 92 ms. The dictation worker stayed on efficiency cores (`pcore_share.dictation` 0.00 to 0.05), and a dictation took 107 ms against 113 ms. Why the tag moved one thread and not the other while the pools still spin is not established.
- **c2, one intra-op thread.** Threads fall from 226 to 36, CPU time during load and warm-up from 67.9 s to 2.95 s, and CPU time in the 5 s after the last transcription from 4.27 s to 0.03 s. The warm-up takes 0.48 s instead of 2.71 s, so the process is ready at 3.33 s instead of 5.98 s. Transcriptions now start and end on a performance core, with a median share of 1.00 in every set, and a dictation takes 64 ms. The working set drops by 57 MB, which the pool threads' stacks would account for (inferred, not measured).
- **c3, lazy resampler sessions.** GPU memory falls from 4110 MB to 3634 MB at load and from 6030 MB to 5632 MB after use. Commit falls by about as much, 403 MB at idle, since WDDM counts GPU allocations as commit.
- **final, one OpenBLAS thread.** Commit at import falls from 662 MB to 51 MB, and by 618 MB at idle. Threads fall from 36 to 17.

**The tag in the final tree.** c1 alone does not show what the tag does once the pools are gone, so a second session ran the final tree against the same tree without the `prefer_performance_cores()` call, 5 processes each (`data/stages/model-cuda-tag`). Untagged, the transcribing threads ran on efficiency cores in most processes (`pcore_share` medians 0.00, ranges 0.00 to 1.00), and every latency rose. A dictation took 112.7 ms against 65.4 ms, the English clip at a new length 108.4 ms against 60.9 ms, and the 110 s clip 558.8 ms against 388.2 ms. Memory, threads and transcripts did not differ. The two commits work together. Without the pools, Windows put the untagged thread on an efficiency core in most processes, and with the pools spinning, the tag did not keep the dictation worker off them.

### CPU provider

CPU-provider latencies vary far more between processes than CUDA's, so the Summary's CPU tables give their ranges, and `tables.md` all metrics.

- **Commits 1 to 5** (`data/stages/model-cpu-reps3`, which also ran 35fb1f7 without the tag). Threads fall from 194 to 62 and commit by 632 MB, from the lazy resampler sessions and one OpenBLAS thread. The dictation worker ran on performance cores in every process (`pcore_share.dictation` 1.00 against 0.00 on da3c189), and the process was ready in 4.73 s against 11.84 s. CPU time in the 5 s after the last transcription stays at 11.4 s, since the pools still spin.
- **Commit 6** (`data/stages/model-cpu-spinning`). CPU time in the 5 s after the last transcription falls from 11.12 s to 0.03 s, and over the run from 1449 s to 920 s. The 2.5 s clip takes 265 ms instead of 871 ms, the English clip 470 ms instead of 918 ms and a dictation 460 ms instead of 1112 ms. The 50 s and 110 s clips take the same time within their ranges, 2779 ms against 2802 ms and 5990 ms against 5751 ms. Why the long clips gain nothing is not established. All 51 transcripts matched.
- **One round of clips** (`data/stages/model-cpu`, da3c189 against 35fb1f7) is the first CPU session, kept for the Limits section's note on noise.

### Settings

The Settings window, with c4 (3998e3a) between them, medians of 5 processes per side:

| metric | base, n=5 | c4, n=5 | final, n=5 |
|---|---:|---:|---:|
| window_s | 0.57 | 0.56 | 0.33 |
| working_set_mb | 83.73 | 85.31 | 54.63 |
| private_working_set_mb | 48.50 | 47.04 | 30.07 |
| commit_mb | 715.94 | 105.07 | 33.32 |
| threads | 28.00 | 9.00 | 7.00 |
| modules | 107.00 | 111.00 | 71.00 |

One OpenBLAS thread alone removes 611 MB of commit and 19 threads from Settings, which imported numpy through the app, and leaves the window's time unchanged. Dispatching before the app's imports removes ONNX Runtime and 40 modules, and the window opens 0.23 s sooner.

### Reproducibility

da3c189 ran twice, in the baseline sessions (`data/baseline`) before any change was written, and as the base side of the stages sessions, with the same package digest. Between the two, its memory, GPU memory and thread counts agree within 0.1%, its CPU times within 5%, and the Settings window's time within 3%. Its startup moved by 0.27 s on CUDA and 0.95 s on the CPU provider, its latencies by up to 8% on CUDA and 5% on the CPU provider, and its overlay frame times by up to 10% at the median and 22% at p95. That drift is why every comparison above sets sides of one session against each other. Transcripts and overlay pixels matched across the sessions (`tables.md`, last section).

## Transcripts

Every model run compares each call's transcript across its processes and sides. On CUDA, all 83 call positions produced the same text in every process of every side, in the stages session and in the tag session. The word error rate against the fixtures' text stayed at 0.00 for English and 0.08 for Portuguese on every side.

On the CPU provider, the one-round session matched in all 22 positions. The three-round session (`data/stages/model-cpu-reps3`) matched in 49 of 51, and in the other 2, of the 50 s and the 110 s clip, da3c189 differed from both later trees, which agreed with each other. The cause is commit 4. On the CPU provider, onnx_asr computes the mel features with numpy, and their matrix product goes through OpenBLAS, whose result depends on its thread count. One thread against 20 changes at most 2.0e-6 in features up to 5.3 (`screens/mel.log`). Every thread count tried, 1, 2, 4, 8 and 20, gives a different rounding, so on da3c189 the features already depended on the machine's number of cores. With one thread they are the same on every machine. Holding the final tree fixed and rebuilding both calls' inputs, `OPENBLAS_NUM_THREADS=20` reproduced da3c189's text in both and `1` the final tree's (`screens/openblas_transcripts.py`, `screens/openblas_transcripts.jsonl`). In both differing transcripts, da3c189 and the final tree make the same number of errors against the fixtures' text, at different words (word error rate 0.2407 and 0.2346 on both sides, `screens/cpu_transcripts.log`). The CUDA path computes its features on the GPU and is not affected.

## Tried and rejected

Each candidate below was measured, and none shipped. The screens ran one config per process in interleaved rounds. Their scripts sit in `data/screens/` next to their raw output and are not part of the harness.

- **One intra-op thread without a high QoS tag.** It removes the pools, but in the `probe_qos.py` screen the transcribing thread then ran on efficiency cores in all 54 calls, at 70 ms median for the English clip at new lengths against 38.6 ms with the tag (`screens/qos.log`). The tag shipped first, so no commit in the series runs slower than its parent.
- **`allow_spinning=0` with the default pools.** It saves as much CPU time as one intra-op thread (0.08 s against 0.06 s of CPU over 5 s of idle), keeps 226 threads, and the English clip at a new length took 111 ms against 90 ms with one thread (`screens/options.log`, rows `nospin` and `t1`).
- **Resampler sessions on the CPU provider.** This is the upper bound for the resampler saving, 466 MB less GPU memory at load (3638 MB against 4104 MB, `screens/options.log`, row `t1_cpures`). The lazy CUDA sessions reach the same 3630 to 3638 MB (`screens/arena.jsonl`, rows `lazyres`) and keep the resampler on the provider that ran it before.
- **cuDNN convolution algorithm search `HEURISTIC` or `DEFAULT`.** ONNX Runtime's default, `EXHAUSTIVE`, stays. With `HEURISTIC` the English clip at a new length took 117 ms against 90 ms, and with `DEFAULT` 696 ms (`screens/options.log`, rows `t1_heur` and `t1_default`). The extra cost of a new input length is not the algorithm search.
- **CUDA arena growth `kSameAsRequested`.** In an 18-clip screen it cut GPU memory after use from 5770 MB to 5152 MB. Over 150 distinct lengths from 0.5 s to 60 s it grew from 5058 MB to 6276 MB, while the default arena stayed between 4614 MB and 4622 MB (`screens/arena.jsonl`, `screens/probe_fragment.py`).
- **`session.use_device_allocator_for_initializers`.** It cut GPU memory at load from 4104 MB to 3370 MB but used more after use, 5978 MB against 5770 MB in the screen and a flat 4826 MB against 4614 MB over the 150 lengths.
- **The decoder on the CPU.** Each emitted token runs the small `decoder_joint` graph once, so a CPU session would save a GPU round trip per token. In `probe_decoder.py` on the final tree, one call took 420 µs at the median on CUDA (p95 596 µs), 1579 µs on the CPU with one thread and 3445 µs with the default pool, with outputs within 3.1e-5 and the same token (`screens/decoder.log`). The English clip makes 32 decoder calls, 16 ms on CUDA, so on the CPU they would take about 50 ms (inferred from the per-call times).
- **Overlay rendering off the transcription's path.** Not built. The bars overlay draws at 60 fps on a thread of the same process while it transcribes, so it could compete for the GIL. In the final tree, the English clip took 65.0 ms with the overlay drawing and 63.6 ms without, and the 20 s clip 98.5 ms against 97.4 ms (`overlay_busy` against `new_length`, stages session), a difference inside the ranges.

## Method

**Machine.** Intel Core i7-14700KF with hyper-threading off, 20 logical CPUs, 0 to 7 performance cores and 8 to 19 efficiency cores by `GetSystemCpuSetInformation`. 64 GB of RAM. NVIDIA GeForce RTX 5080 with 16 GB, driver 617.14. Windows 11 Pro build 26200, power plan "Ultimate Performance". Python 3.12.14, onnxruntime-gpu 1.31.0, onnx-asr 0.12.0, numpy 2.5.3, Pillow 12.3.0, httpx2 2.13.1. Model `parakeet-tdt-0.6b-v3`, fp32 on CUDA and int8 on the CPU provider, read from the installed app's `models` folder.

**Sides.** Each side is a source tree exported with `git archive` and run with this repository's virtual environment through `PYTHONPATH`. Every run records its side's commit (`sha`), and every run in `data/stages` also records a digest of the package it ran (`source_digest`). Two experiment trees differ from 35fb1f7 by one change each, which their `sha` field states: `notag` drops the `prefer_performance_cores()` call, and `stopspin` sets `session.force_spinning_stop` on the CPU provider. The harness runs one process at a time and alternates the sides (base, c1, c2, c3, final, base, ...), so drift and background load reach every side alike. One warm-up process per side runs first and is discarded. It also compiles each exported tree's `.pyc` files.

**What a model run does.** A fresh process imports `parakeet_dictate.app` and loads and warms the model through `load_transcriber` and `warm_up` at 44.1 kHz, as the app does. Then it runs these sets:

- `new_length`: 5 rounds of six clips built from the English and Portuguese fixtures (2.5 s, 6.0 s, 6.6 s, 20 s, 50 s and 110 s), each call with a unique silent tail, so no input length repeats, as in real dictation. `first_ms` is each clip's first call, and `ms` the other four.
- `same_length`: each of three clips 5 times with identical input, first call excluded.
- `overlay_busy`: transcriptions at new lengths while a thread renders bars overlay frames at 60 fps, as the app does while it transcribes.
- `dictation`: 20 hold-to-talk dictations through `Dictation`'s controller thread, with fixture audio in place of the recorder and a paste function that records the text, timed from key-up to paste. No keyboard hook is installed and nothing touches the clipboard.
- `dictation_polish_mock`: 6 dictations with post-processing through a local mock transport that answers like GLM, with a per-app rule for the foreground executable. It measures local overhead only and sends nothing over the network.
- Idle: 30 s without work, with CPU time over the first 5 s and the next 25 s.

Memory is sampled after import, load, warm-up, the new-length set, the dictation sets and 30 s of idle. `working_set_mb` is RAM resident now. `private_working_set_mb` is the resident part that belongs to this process alone. `commit_mb` is private bytes (`PrivateUsage`), the memory the process has committed, resident or not, which counts against the system commit limit. On this WDDM driver, the process's GPU allocations also count as commit, which is why commit moves with GPU memory. `gpu_dedicated_mb` is the WDDM "GPU Process Memory\Dedicated Usage" counter, the per-process number Task Manager shows. CPU time is the process's user plus kernel time. Threads come from a Toolhelp32 snapshot. `pcore_share` is the share of calls whose thread started and ended on a performance core. `wer` is the word error rate against the fixtures' reference text, and every run compares each call's transcript across processes and sides.

**Settings runs** start `.venv\Scripts\pythonw.exe -m parakeet_dictate --settings` with the side on `PYTHONPATH` and time it from process start to a visible window titled "Parakeet Dictate — Settings". After 1.5 s they read the counters of the process that owns the window and close it with `WM_CLOSE`. The venv's `pythonw.exe` is a launcher that starts the base interpreter as a child. The numbers are the child's, and the launcher's own working set (8.1 MB) is recorded next to them. **Overlay runs** render a scripted 12.5 s sequence of frames per style and scale in one process, and hash every frame's pixels.

**Commands.** From the repository root, with `MODEL` set to the model folder and each side's tree exported as described:

```powershell
uv run python scripts/bench_resources.py model --side base=<da3c189>/src --side final=src --model-dir $MODEL --runs 5 --warmup 1 --reps 5 --out <folder>
uv run python scripts/bench_resources.py model --provider cpu --side base=<da3c189>/src --side final=src --model-dir $MODEL --runs 3 --warmup 1 --reps 1 --out <folder>
uv run python scripts/bench_resources.py settings --side base=<da3c189>/src --side final=src --runs 5 --warmup 1 --out <folder>
uv run python scripts/bench_resources.py overlay --side base=<da3c189>/src --side final=src --runs 5 --warmup 1 --out <folder>
uv run python scripts/bench_resources.py compare <folder> <folder> --before-side base --after-side final
uv run python scripts/bench_resources.py summary <folder> --markdown
```

Each folder's `*-meta.json` holds the exact command line, the sides' paths and commits, and the environment. `docs/perf/tables.sh > docs/perf/tables.md` prints every table in `tables.md` again from the data.

## Limits

- **No cold start.** Every process read the model from a file cache that earlier runs had filled, so startup times are those of a new process with a warm cache. A first start after boot also reads the fp32 model's 2.5 GB from disk, and these changes do not touch that read.
- **A shared machine.** The installed app ran through every session, idle, holding about 4.6 GB of GPU memory, and the machine stayed in use. Interleaving spreads that load over all sides of a session, but numbers from different sessions are not comparable. In particular, where Windows put an untagged thread varied from session to session, so base latencies differ between the baseline folder, the stages session and the screens.
- **The harness is not the tray app.** The model runs in a console process the harness starts, and the app runs in `pythonw.exe` without a console. Windows weighs process and window state when it schedules, so the untagged app may land on other cores than the harness did. The tag sets the transcribing thread's level in both.
- **No devices.** No microphone, keyboard hook, clipboard or paste took part. Dictations ran through `Dictation`'s controller thread with fixture audio and a paste function that records text. `scripts/e2e_check.py` did not run, because it needs the tray app stopped and pastes into the foreground window, and the installed app was in use.
- **Remote providers.** Post-processing ran against a local mock of the GLM API, so no number here includes network or provider time. No change touches the provider path.
- **The `parakeet-dictate --settings` script.** The tray icon opens Settings with `pythonw -m parakeet_dictate --settings`, which takes the new path. The console script that the project README mentions, `uv run parakeet-dictate --settings`, enters through `app.main` and still imports the app first. It opens as before, without the saving.
- **Saving Settings.** Saving was not exercised against a running tray app, because it would have reloaded the installed one. Tests cover both branches of `apply_settings` under an event name of their own, and the tray app's reload on that event.
- **macOS.** The Mac client was neither changed nor measured. It uploads 16 kHz audio, which needs no resampler session, and the Windows server it talks to transcribes through the same `Transcriber`, so its request threads get the same tag.
- **GPU memory.** The numbers are WDDM's per-process counters. Under WDDM, `nvidia-smi` reports no per-process usage, so they could not be cross-checked against NVML.
- **The CPU provider's latencies** vary far more between processes than CUDA's, since every transcription spreads over all 20 cores and meets whatever else the machine runs. A single round of clips with 3 processes per side (`data/stages/model-cpu`) showed the 20 s clip slower after the changes, 2232 ms against 1590 ms with the overlay drawing, and the sessions with 3 rounds did not repeat it. Read CPU-provider latencies with their ranges in `tables.md`.
- **A new microphone rate.** The warm-up runs at the default input device's rate and opens that rate's resampler session. A dictation at another rate, after switching to an input device with a different default rate while the app runs, opens that rate's session first. In `probe_rescreate.py` on the final tree, the first 32 kHz call took 2.9 to 7.1 ms longer than the second (3 processes, `screens/rescreate.jsonl`). Both resample to the same length, so the difference is the session's opening and its first run. The base rows in that file ran untagged with the pools spinning, so their absolute times are not comparable.
- **Private memory after warm-up.** On CUDA, the model process's private working set grows from 431 MB after load to 1041 MB after the two warm-up transcriptions, in every side. This work did not break that 610 MB down. The CUDA JIT cache received no writes during the sessions, so it is not kernels compiled from PTX at each start.
- **Tradeoffs.** Tagged transcriptions run on performance cores, which draw more power than efficiency cores while they work, and a transcription takes tens of milliseconds of one core. With one OpenBLAS thread, large matrix products would run slower. The app's are small, as item 4 of "What changed" measures.
- **Not tried.** fp16 or int8 on CUDA, TensorRT and smaller models change the model's numerics or the model itself, which this work ruled out.
