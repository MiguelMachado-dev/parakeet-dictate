# Next steps

Where the performance and memory work stood when it paused on 2026-10-14, and how to pick it up. [`README.md`](README.md) holds the results and the method, and [`decisions.tsv`](decisions.tsv) every decision with its evidence.

## State

The work lives on branch `t3/performance-memory-benchmarks`, in the worktree `C:\Users\migue\.t3\worktrees\parakeet-dictate\t3-073be6d8`, on top of da3c189. Nothing was pushed or merged. The installed app in `C:\Users\migue\www\parakeet-dictate` was not changed; it ran through every measurement, and its `models` folder was only read.

Commits, oldest first:

| Commit | What it does |
|---|---|
| b2bef06, faac1d0, 97becdb, 90dfc39 | The benchmark harness, `scripts/bench_resources.py` and `scripts/winstats.py`, and its fixes. |
| e94cfd4 | The baseline measurements of da3c189, `data/baseline`, taken before any change. |
| fcb4a74 | 1. Tags the transcribing thread high QoS, so Windows keeps it on performance cores. |
| 4fcc2d0 | 2. One intra-op thread per CUDA session, instead of a spinning pool of a thread per core. |
| f66a11a | 3. Opens resampler sessions lazily, one per sample rate the app receives, instead of seven at load. |
| 3998e3a | 4. `OPENBLAS_NUM_THREADS=1`, unless the environment sets it. |
| b50b4aa | 5. `python -m parakeet_dictate --settings` opens Settings without importing the app. |
| 35fb1f7, c052e96, dd45218 | Harness: the cores each call ran on, p95 and counts in tables, `--only`, `summary --markdown`, overlay pixel checks. |
| 38e4a05 | 6. `session.force_spinning_stop` on the CPU provider's sessions. |
| e88bda8 | Tests for `apply_settings`, which commit 5 touched. |
| The commit after e88bda8 | This folder: the report, the stages data, the screens and the decision log. |

The full suite passed at e88bda8, 311 tests, 8 of them new: 6 in `tests/test_transcriber.py` and 2 in `tests/test_settings_window.py`. The tests of the QoS tag, of the lazy resampler and of `apply_settings` each failed against a deliberately broken version of the code they cover.

## Results in one paragraph

On CUDA, da3c189 against the final tree, 5 processes each: ready 5.91 s to 3.30 s, CPU time for load and warm-up 67.5 s to 2.9 s, CPU in the 5 s after use 4.47 s to 0.05 s, threads 226 to 17, GPU memory after use 6038 MB to 5628 MB, commit 8483 MB to 7395 MB, dictation key-up to paste 113 ms to 68 ms at the median, all 83 transcripts identical. Settings opens in 0.33 s instead of 0.57 s with 33 MB of commit instead of 716 MB. The overlay renders identical pixels in the same time with 4 threads instead of 23. On the CPU provider, commits 1 to 5 cut threads from 194 to 62 and commit by 632 MB, and commit 6 cut CPU after use from 11.12 s to 0.03 s and dictation latency from 1112 ms to 460 ms. Commit 4 changes the rounding of the CPU provider's mel features, which changed the words, not the error count, of 2 long transcripts out of 51.

## Next steps, in order

1. **The CPU provider, da3c189 against 38e4a05.** The session was stopped after 2 measured processes per side. Its partial data, ready 6.7 to 6.8 s against 3.8 to 4.0 s, 194 to 195 threads against 62 and 3642 to 3643 MB of commit against 3011 MB, is in `%TEMP%\parakeet-perf\model-cpu-final-partial`, outside the repository. Export both trees (below) and run, with nothing else heavy on the machine, about 30 minutes:

   ```powershell
   uv run python scripts/bench_resources.py model --provider cpu --side base=<da3c189>/src --side final=<38e4a05>/src --model-dir C:/Users/migue/www/parakeet-dictate/models/parakeet-tdt-0.6b-v3 --runs 5 --warmup 1 --reps 3 --out docs/perf/data/stages/model-cpu-final
   ```

   Then add `table compare $D/stages/model-cpu-final $D/stages/model-cpu-final --before-side base --after-side final` to `tables.sh`, regenerate `tables.md`, and let that table replace the two CPU tables in the README's Summary. Expect 2 differing transcripts, the OpenBLAS rounding the README explains.
2. **Check the installed app after integration.** The harness is not the tray app. After installing the branch, compare the app's own counters with those it showed on da3c189 while idle (pid 34960: 233 threads, 4602 MB of dedicated GPU memory, 1298 MB working set, 6990 MB commit), dictate with the microphone, save Settings during and after a dictation, and run `scripts/e2e_check.py`. That script needs the tray app stopped and pastes into the foreground window, so it waits for a moment the machine is free.
3. **Optional: the private memory warm-up adds.** On CUDA, the model process's private working set grows by about 610 MB during the two warm-up transcriptions, on every side. A VMMap or heap breakdown would show whether any of it is avoidable.
4. **Optional: the console script.** `uv run parakeet-dictate --settings` enters through `app.main` and still imports the app before Settings. Pointing the script entry at a dispatcher like `__main__`'s would give it the same saving as the tray icon's path.
5. **Optional: a cold start.** Every measurement had the model in the file cache. A first start after boot was not measured.

## Rebuilding the sides

Each side is a tree exported from a commit, with a `SOURCE_SHA` file that the harness records:

```bash
mkdir -p "$TEMP/parakeet-perf/sides/final2"
git archive 38e4a05 src pyproject.toml | tar -x -C "$TEMP/parakeet-perf/sides/final2"
git rev-parse 38e4a05 > "$TEMP/parakeet-perf/sides/final2/SOURCE_SHA"
```

The base tree is da3c189 exported the same way. The experiment trees were 35fb1f7 with one change each: `notag` without the `prefer_performance_cores()` call in `Transcriber.transcribe`, and `stopspin` with the two `force_spinning_stop` lines that 38e4a05 later committed.

## Rules the work kept

- Never modify, update or restart the installed app. Read its models only by explicit path.
- Never run the full app or `scripts/e2e_check.py` while the installed app runs, never signal its named events, and never force-kill it. Stop it only with `--quit` after confirming it is idle, and restore it from the Desktop shortcut.
- Keep secrets out of reports and data. The benchmark's post-processing runs against a local mock, never a real provider.
- One harness session at a time, alternating sides, with a warm-up process per side. Do not edit `scripts/bench_resources.py` while a session runs, since every child process reads it again.
- No push, pull request or merge from this branch. Integrating and installing is a separate, deliberate step.
