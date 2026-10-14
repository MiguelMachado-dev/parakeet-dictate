"""Measure what the app costs: RAM, commit, GPU memory, threads, CPU, startup, and latency.

model      Fresh processes that import the app as it starts, load and warm the model through the app's own
           code, transcribe fixture audio at 44.1 kHz, dictate through Dictation's controller with a paste that
           only records the text, and then sit idle. No keyboard hook, clipboard, sound, or network: the
           post-processing set talks to a local mock transport.
settings   Starts the real `pythonw -m parakeet_dictate --settings`, times its window, reads its memory and
           loaded modules, and closes it. Refuses to run while a Settings window is open.
overlay    Renders a scripted sequence of overlay frames in-process and times each one, with a hash of
           every frame's pixels; also times the level meter on fixture audio.
summary    Prints the medians of a results folder, per side.

Each side is a checkout's src folder, run with this venv through PYTHONPATH, so one command alternates
between two versions of the app: --side base=<checkout>/src --side opt=src. Sides run as base, opt,
base, opt, so drift and background load reach both alike. Results land in --out as one JSON file per
process, with the source tree's digest, so the data proves which code each number came from.
"""

import argparse
import ctypes
import hashlib
import json
import os
import statistics
import subprocess
import sys
import threading
import time
from ctypes import wintypes
from pathlib import Path

import winstats

SCRIPTS = Path(__file__).resolve().parent
ROOT = SCRIPTS.parent
FIXTURES = ROOT / "tests" / "fixtures"
RATE = 44_100
SETTINGS_TITLE = "Parakeet Dictate — Settings"
SETTINGS_MUTEX = "Local\\parakeet-dictate-settings"
# Per clip: seconds of speech, built from the fixtures. Real dictations in the log run 2 to 110 s.
CLIPS = {"short": 2.5, "en": None, "pt": None, "mid": 20.0, "long": 50.0, "xlong": 110.0}
SAME_LENGTH_CLIPS = ("short", "en", "mid")
OVERLAY_CLIPS = ("short", "en", "mid")
DICTATION_CLIPS = ("short", "en", "pt", "mid")
GAP_S = 0.4
NOISE = 3e-4


# --- child: model ---------------------------------------------------------------------------------------


def snapshot(gpu: "winstats.GpuMemory | None" = None) -> dict:
    row = {**winstats.memory(), "threads": winstats.threads(), "since_start_s": winstats.seconds_since_start()}
    if gpu is not None:
        row.update(gpu.read())
    return row


def source_digest(package: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(package.rglob("*.py")):
        digest.update(path.relative_to(package).as_posix().encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()[:16]


def build_clips(np, read_fixture) -> dict:
    """Speech at RATE for each clip in CLIPS. Silence between repeats carries faint noise, as a room does."""
    rng = np.random.default_rng(7)
    en, pt = (upsample(np, read_fixture(name)) for name in ("en", "pt"))
    gap = (rng.standard_normal(round(GAP_S * RATE)) * NOISE).astype(np.float32)
    unit = np.concatenate([en, gap, pt, gap])
    clips = {"en": en, "pt": pt}
    for name, seconds in CLIPS.items():
        if seconds is not None:
            clips[name] = np.tile(unit, int(seconds * RATE // len(unit)) + 1)[: round(seconds * RATE)].copy()
    return clips


def upsample(np, samples):
    """FFT resampling from 16 kHz to RATE, kept here so both sides of a comparison hear the same input."""
    n_out = round(len(samples) * RATE / 16_000)
    spectrum = np.fft.rfft(samples)
    padded = np.zeros(n_out // 2 + 1, complex)
    padded[: len(spectrum)] = spectrum
    return (np.fft.irfft(padded, n_out) * (n_out / len(samples))).astype(np.float32)


def run_model(args: argparse.Namespace) -> dict:
    gpu = winstats.GpuMemory()
    phases = {"python": snapshot(gpu)}
    t = time.perf_counter()
    import parakeet_dictate
    import parakeet_dictate.app as app
    import_s = time.perf_counter() - t
    phases["imported"] = snapshot(gpu)

    import httpx2
    import numpy as np

    from parakeet_dictate.app_rules import AppRule
    from parakeet_dictate.config import Config, PostprocessConfig
    from parakeet_dictate.polish import create_polisher
    from parakeet_dictate.recorder import Audio
    from parakeet_dictate.reducer import HotkeyDown, HotkeyUp
    from parakeet_dictate.transcriber import SharedModel, load_transcriber

    package = Path(parakeet_dictate.__file__).resolve().parent
    result = {
        "kind": "model",
        "provider": args.provider,
        "source": str(package),
        "source_digest": source_digest(package),
        "import_s": import_s,
    }

    t = time.perf_counter()
    transcriber = load_transcriber(Path(args.model_dir), args.provider)
    result["load_s"] = time.perf_counter() - t
    result["label"] = transcriber.label
    phases["loaded"] = snapshot(gpu)
    t = time.perf_counter()
    transcriber.warm_up(RATE)
    result["warm_up_s"] = time.perf_counter() - t
    phases["warm"] = snapshot(gpu)
    model = SharedModel()
    model.load(transcriber)

    def read_fixture(name: str):
        import wave
        with wave.open(str(FIXTURES / f"{name}.wav")) as f:
            return np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768

    clips = build_clips(np, read_fixture)
    tails = iter(range(1, 1_000_000))
    rng = np.random.default_rng(11)

    def fresh(name: str):
        """The clip with a tail no earlier call had, so its length is new, as every real dictation's is."""
        tail = (rng.standard_normal(round((0.05 + 0.0137 * next(tails)) * RATE)) * NOISE).astype(np.float32)
        return np.concatenate([clips[name], tail])

    sets: dict[str, list[dict]] = {}

    def transcribe(set_name: str, name: str, samples) -> None:
        row = {"clip": name, "seconds": len(samples) / RATE}
        # Which logical CPU the call started and ended on: this CPU mixes performance and efficiency cores.
        core = kernel32.GetCurrentProcessorNumber()
        t0 = time.perf_counter()
        try:
            row["text"] = model.transcribe(Audio(samples, RATE))
        except Exception as e:
            row["error"] = f"{type(e).__name__}: {e}"
        row["ms"] = (time.perf_counter() - t0) * 1000
        row["cores"] = [core, kernel32.GetCurrentProcessorNumber()]
        sets.setdefault(set_name, []).append(row)

    for _ in range(args.reps):
        for name in CLIPS:
            transcribe("new_length", name, fresh(name))
    phases["after_new_length"] = snapshot(gpu)

    for name in SAME_LENGTH_CLIPS:
        samples = fresh(name)
        for _ in range(args.reps):
            transcribe("same_length", name, samples)
    phases["after_same_length"] = snapshot()

    stop_drawing = threading.Event()
    drawn = overlay_loop_in_thread(stop_drawing)
    for _ in range(max(1, args.reps - 1)):
        for name in OVERLAY_CLIPS:
            transcribe("overlay_busy", name, fresh(name))
    stop_drawing.set()
    result["overlay_busy_frames"] = drawn()
    phases["after_overlay_busy"] = snapshot()

    plain = Config(model_dir=Path(args.model_dir), provider=args.provider)
    sets["dictation"] = dictate(app, model, plain, None, lambda: None, [fresh(n) for n in DICTATION_CLIPS * args.reps],
                                HotkeyDown, HotkeyUp)
    phases["after_dictation"] = snapshot(gpu)

    rule = "Translate the transcript to English."
    mock = MockProvider(httpx2, rule)
    polishing = Config(
        model_dir=Path(args.model_dir), provider=args.provider,
        postprocess=PostprocessConfig(endpoint="https://mock.invalid/api/coding/paas/v4/chat/completions"),
        app_rules=(AppRule(name="English", instructions=rule, apps=("Slack.exe",)),),
    )
    polisher = create_polisher(polishing.postprocess, "mock-key", transport=httpx2.MockTransport(mock.handle))
    sets["dictation_polish_mock"] = dictate(app, model, polishing, polisher, lambda: "Slack.exe",
                                            [fresh(n) for n in ("short", "en")] * 3, HotkeyDown, HotkeyUp)
    polisher.close()
    result["mock_provider"] = {"posts": mock.posts, "posts_with_rule": mock.with_rule, "preconnects": mock.preconnects}

    idle = snapshot()
    time.sleep(5)
    phases["idle_5s"] = snapshot()
    time.sleep(25)
    phases["idle_30s"] = snapshot(gpu)
    result["idle_cpu_s"] = {
        "0_5s": phases["idle_5s"]["cpu_s"] - idle["cpu_s"],
        "5_30s": phases["idle_30s"]["cpu_s"] - phases["idle_5s"]["cpu_s"],
    }
    result["phases"] = phases
    result["sets"] = sets
    gpu.close()
    return result


class MockProvider:
    """Answers the GLM client like Z.ai would, at once, so the timing is the app's own work."""

    def __init__(self, httpx2, rule: str) -> None:
        self._httpx2, self._rule = httpx2, rule
        self.posts = self.with_rule = self.preconnects = 0

    def handle(self, request):
        if request.method == "OPTIONS":
            self.preconnects += 1
            return self._httpx2.Response(204)
        body = json.loads(request.content)
        self.posts += 1
        self.with_rule += self._rule in body["messages"][0]["content"]
        raw = body["messages"][1]["content"].removeprefix("<transcript>").removesuffix("</transcript>")
        return self._httpx2.Response(200, json={
            "choices": [{"message": {"content": f"[polished] {raw}"}, "finish_reason": "stop"}],
            "usage": {"completion_tokens_details": {"reasoning_tokens": 0}},
        })


def dictate(app, model, config, polisher, foreground, clips, HotkeyDown, HotkeyUp) -> list[dict]:
    """Hold-to-talk dictations through Dictation's controller thread, timed from key-up to the paste call."""
    pasted: list[tuple[float, str]] = []
    done = threading.Event()

    class ClipRecorder:
        device_name = "fixture"
        clip = None

        def start(self) -> None:
            pass

        def stop(self):
            from parakeet_dictate.recorder import Audio
            return Audio(self.clip, RATE)

        def discard(self) -> None:
            pass

    def paste(text: str) -> None:
        pasted.append((time.perf_counter(), text))
        done.set()

    recorder = ClipRecorder()
    dictation = app.Dictation(config, recorder=recorder, transcriber=model, paste=paste, cues=app.Cues(False),
                              polisher=polisher, polish_enabled=lambda: polisher is not None, foreground_app=foreground)
    # The controller only; the keyboard hook is never installed.
    dictation._controller.start()
    rows = []
    for samples in clips:
        recorder.clip = samples
        done.clear()
        dictation.post(HotkeyDown(time.perf_counter() - 2.0))
        released = time.perf_counter()
        dictation.post(HotkeyUp(released))
        row = {"seconds": len(samples) / RATE}
        if done.wait(30):
            row["ms"] = (pasted[-1][0] - released) * 1000
            row["text"] = pasted[-1][1]
        else:
            row["error"] = "no paste within 30 s"
        rows.append(row)
    dictation._events.put(None)
    dictation._controller.join(10)
    dictation._worker.shutdown(wait=True)
    dictation._polisher.shutdown(wait=True)
    return rows


def overlay_loop_in_thread(stop: threading.Event):
    """The overlay thread's per-frame work at 60 fps, without a window: scene, bars renderer, premultiply."""
    import numpy as np

    from parakeet_dictate.overlay import premultiplied_bgra
    from parakeet_dictate.overlay_frame import FPS, Phase
    from parakeet_dictate.overlay_scene import Placement, Scene, Show
    from parakeet_dictate.styles import STYLES

    frames = [0]

    def loop() -> None:
        render = STYLES["bars"]
        scene = Scene()
        start = time.perf_counter()
        scene.apply(Show(Phase.TRANSCRIBING), start - 1.0, Placement(1080, 1272, 400, 120, 1.0))
        levels, spectrum = np.linspace(0, 1, 96, dtype=np.float32), np.full(24, 0.4, np.float32)
        due = start
        while not stop.is_set():
            shown = scene.frame(time.perf_counter(), levels, spectrum)
            premultiplied_bgra(np.asarray(render(shown[0])))
            frames[0] += 1
            due += 1 / FPS
            stop.wait(max(0.0, due - time.perf_counter()))

    threading.Thread(target=loop, name="overlay", daemon=True).start()
    return lambda: frames[0]


# --- parent ---------------------------------------------------------------------------------------------


def parse_sides(values: list[str]) -> list[tuple[str, Path]]:
    sides = []
    for value in values or [f"current={ROOT / 'src'}"]:
        name, _, src = value.partition("=")
        src = Path(src).resolve()
        if not (src / "parakeet_dictate" / "__init__.py").is_file():
            raise SystemExit(f"--side {value}: no parakeet_dictate package in {src}")
        sides.append((name, src))
    return sides


def side_sha(src: Path) -> str:
    marker = src.parent / "SOURCE_SHA"
    if marker.is_file():
        return marker.read_text().strip()
    result = subprocess.run(["git", "-C", str(src), "rev-parse", "HEAD"], capture_output=True, text=True)
    dirty = subprocess.run(["git", "-C", str(src), "status", "--porcelain", "--", "."], capture_output=True, text=True)
    return result.stdout.strip() + ("+dirty" if dirty.stdout.strip() else "")


def environment() -> dict:
    """Hardware and versions, for the record. Never the process environment: it can hold API keys."""
    import platform
    from importlib.metadata import version

    gpu = subprocess.run(["nvidia-smi", "--query-gpu=name,driver_version,memory.total", "--format=csv,noheader"],
                         capture_output=True, text=True)
    return {
        "python": sys.version.split()[0],
        "platform": platform.platform(),
        "cpu": platform.processor(),
        "logical_cpus": os.cpu_count(),
        "gpu": gpu.stdout.strip(),
        "packages": {name: version(name) for name in ("onnxruntime-gpu", "onnx-asr", "numpy", "pillow", "httpx2")},
    }


def machine_load() -> dict:
    gpu = subprocess.run(["nvidia-smi", "--query-gpu=utilization.gpu,memory.used,pstate", "--format=csv,noheader"],
                         capture_output=True, text=True)
    return {"gpu": gpu.stdout.strip()}


def child_env(src: Path) -> dict:
    env = dict(os.environ)
    env["PYTHONPATH"] = str(src)
    env["PYTHONUTF8"] = "1"
    return env


def run_children(args: argparse.Namespace, kind: str, measure) -> None:
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    sides = parse_sides(args.side)
    meta = {"environment": environment(), "sides": {name: {"src": str(src), "sha": side_sha(src)} for name, src in sides},
            "argv": sys.argv[1:]}
    (out / f"{kind}-meta.json").write_text(json.dumps(meta, indent=2), encoding="utf-8")
    plan = [(name, src, "warmup") for _ in range(args.warmup) for name, src in sides]
    plan += [(name, src, "measured") for _ in range(args.runs) for name, src in sides]
    for index, (name, src, role) in enumerate(plan):
        busy0 = winstats.system_cpu()
        load = machine_load()
        t = time.perf_counter()
        row = measure(args, src, out / f"{kind}-{index:02d}-{name}-{role}")
        busy1 = winstats.system_cpu()
        row.update(side=name, sha=meta["sides"][name]["sha"], role=role, index=index, wall_s=time.perf_counter() - t,
                   machine_before=load, machine_cpu_busy=(busy1[0] - busy0[0]) / max(1, busy1[1] - busy0[1]))
        path = out / f"{kind}-{index:02d}-{name}-{role}.json"
        path.write_text(json.dumps(row, indent=1, ensure_ascii=False), encoding="utf-8")
        print(f"[{index + 1}/{len(plan)}] {kind} {name} {role}: {headline(row)}", flush=True)


def measure_model(args: argparse.Namespace, src: Path, stem: Path) -> dict:
    json_out = stem.with_suffix(".child.json")
    command = [sys.executable, str(Path(__file__)), "model-child", "--model-dir", args.model_dir,
               "--provider", args.provider, "--reps", str(args.reps), "--json-out", str(json_out)]
    with open(stem.with_suffix(".log"), "w", encoding="utf-8") as log:
        code = subprocess.run(command, env=child_env(src), cwd=src.parent, stdout=log, stderr=subprocess.STDOUT).returncode
    row = json.loads(json_out.read_text(encoding="utf-8")) if json_out.is_file() else {}
    json_out.unlink(missing_ok=True)
    row["exit_code"] = code
    return row


def headline(row: dict) -> str:
    if row.get("exit_code"):
        return f"FAILED with exit code {row['exit_code']}; see the .log next to it"
    if row.get("kind") == "model":
        warm, idle = row["phases"]["warm"], row["phases"]["idle_30s"]
        errors = sum("error" in r for rows in row["sets"].values() for r in rows)
        return (f"ready {warm['since_start_s']:.1f} s, WS {idle['working_set_mb']:.0f} MB, commit {idle['commit_mb']:.0f} MB, "
                f"GPU {idle['gpu_dedicated_mb']:.0f} MB, threads {idle['threads']}, errors {errors}, exit {row['exit_code']}")
    if row.get("kind") == "settings":
        return (f"window {row.get('window_s', float('nan')):.2f} s, WS {row.get('working_set_mb', 0):.0f} MB, "
                f"commit {row.get('commit_mb', 0):.0f} MB, onnxruntime loaded {row.get('onnxruntime_loaded')}, exit {row.get('exit_code')}")
    if row.get("kind") == "overlay":
        return ", ".join(f"{k} p50 {v['p50_ms']:.2f} ms" for k, v in row["render"].items())
    return json.dumps(row)[:200]


# --- settings window ------------------------------------------------------------------------------------

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user32.EnumWindows.argtypes = [WNDENUMPROC, wintypes.LPARAM]
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.GetDpiForWindow.argtypes = [wintypes.HWND]
user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
kernel32.OpenMutexW.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.LPCWSTR]
kernel32.OpenMutexW.restype = wintypes.HANDLE
WM_CLOSE = 0x0010
SYNCHRONIZE = 0x00100000


def settings_window(pids: set[int]) -> tuple[int, int] | None:
    """The Settings window of one of pids, and the process that owns it."""
    found = []

    @WNDENUMPROC
    def check(hwnd, unused):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value in pids and user32.IsWindowVisible(hwnd):
            title = ctypes.create_unicode_buffer(256)
            user32.GetWindowTextW(hwnd, title, 256)
            if title.value == SETTINGS_TITLE:
                found.append((hwnd, owner.value))
                return False
        return True

    user32.EnumWindows(check, 0)
    return found[0] if found else None


def measure_settings(args: argparse.Namespace, src: Path, stem: Path) -> dict:
    held = kernel32.OpenMutexW(SYNCHRONIZE, False, SETTINGS_MUTEX)
    if held:
        kernel32.CloseHandle(held)
        raise SystemExit("A Settings window is open; close it first. Starting another would bring that one forward.")
    pythonw = Path(sys.executable).with_name("pythonw.exe")
    t = time.perf_counter()
    process = subprocess.Popen([str(pythonw), "-m", "parakeet_dictate", "--settings"], env=child_env(src), cwd=src.parent)
    row = {"kind": "settings"}
    try:
        while (found := settings_window({process.pid, *winstats.child_pids(process.pid)})) is None:
            if process.poll() is not None or time.perf_counter() - t > 30:
                row["error"] = f"no window (exit code {process.poll()})"
                return row
            time.sleep(0.005)
        row["window_s"] = time.perf_counter() - t
        hwnd, owner = found
        time.sleep(args.settle)
        row.update(winstats.memory(owner), threads=winstats.threads(owner))
        rect = wintypes.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
        row["dpi"], row["window_size"] = user32.GetDpiForWindow(hwnd), [rect.right - rect.left, rect.bottom - rect.top]
        if owner != process.pid:
            launcher = winstats.memory(process.pid)
            row["launcher_working_set_mb"], row["launcher_commit_mb"] = launcher["working_set_mb"], launcher["commit_mb"]
        loaded = winstats.modules(owner)
        row["modules"] = len(loaded)
        row["onnxruntime_loaded"] = any(name.casefold().startswith("onnxruntime") for name in loaded)
        row["cuda_loaded"] = any(name.casefold().startswith(("cudart", "cublas", "cudnn")) for name in loaded)
        row["portaudio_loaded"] = any("portaudio" in name.casefold() for name in loaded)
        row["openblas_loaded"] = any("openblas" in name.casefold() for name in loaded)
        user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
        row["exit_code"] = process.wait(15)
    finally:
        if process.poll() is None:
            for pid in winstats.child_pids(process.pid):
                subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"], capture_output=True)
            process.terminate()
    return row


# --- overlay and meter ----------------------------------------------------------------------------------


def run_overlay(args: argparse.Namespace) -> dict:
    # The package first, as in the app: it may configure numpy's environment before numpy loads.
    import parakeet_dictate
    import numpy as np

    from parakeet_dictate.meter import LevelMeter
    from parakeet_dictate.overlay import premultiplied_bgra
    from parakeet_dictate.overlay_frame import FPS, Phase
    from parakeet_dictate.overlay_scene import Finish, Scene, Show, Vanish, place
    from parakeet_dictate.styles import STYLES

    def read_fixture(name: str):
        import wave
        with wave.open(str(FIXTURES / f"{name}.wav")) as f:
            return np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768

    speech = build_clips(np, read_fixture)["mid"]
    # (time, command) per dictation: hold then paste with polishing; hands-free then cancel; then a failure.
    script = [
        (0.0, Show(Phase.RECORDING)), (3.0, Show(Phase.TRANSCRIBING)), (3.3, Show(Phase.POLISHING)), (4.4, Finish(Phase.DONE)),
        (6.0, Show(Phase.RECORDING, hands_free=True)), (8.0, Finish(Phase.CANCELLED)),
        (9.5, Show(Phase.RECORDING)), (10.5, Show(Phase.TRANSCRIBING)), (10.9, Finish(Phase.FAILED)),
        (12.0, Show(Phase.RECORDING)), (12.1, Vanish()),
    ]
    end_s = 12.5
    package = Path(parakeet_dictate.__file__).resolve().parent
    result = {"kind": "overlay", "source": str(package), "source_digest": source_digest(package), "render": {}, "hashes": {}}
    for style in ("bars", "fluid"):
        for scale in (1.0, 1.5):
            placement = place((0, 0, 2560, 1400), round(96 * scale))
            times, digest = [], hashlib.sha256()
            for repeat in range(args.repeats):
                meter, scene, fed, commands = LevelMeter(RATE), Scene(), 0, list(script)
                for i in range(int(end_s * FPS)):
                    now = i / FPS
                    while commands and commands[0][0] <= now:
                        scene.apply(commands.pop(0)[1], now, placement)
                    until = min(len(speech), round(now * RATE))
                    while fed < until:
                        meter.feed(speech[fed : fed + 441])
                        fed += 441
                    levels, spectrum = meter.snapshot()
                    t0 = time.perf_counter()
                    shown = scene.frame(now, levels, spectrum)
                    if shown is None:
                        continue
                    pixels = premultiplied_bgra(np.asarray(STYLES[style](shown[0])))
                    times.append((time.perf_counter() - t0) * 1000)
                    if repeat == 0:
                        digest.update(pixels.tobytes())
            key = f"{style}@{scale:g}"
            ms = np.array(times)
            result["render"][key] = {"frames": len(ms), "p50_ms": float(np.percentile(ms, 50)),
                                     "p95_ms": float(np.percentile(ms, 95)), "mean_ms": float(ms.mean())}
            result["hashes"][key] = digest.hexdigest()

    calls, block = [], 441
    meter = LevelMeter(RATE)
    for repeat in range(args.repeats):
        meter.reset()
        for offset in range(0, len(speech), block):
            t0 = time.perf_counter()
            meter.feed(speech[offset : offset + block])
            calls.append((time.perf_counter() - t0) * 1e6)
    levels, spectrum = meter.snapshot()
    us = np.array(calls)
    result["meter"] = {"calls": len(us), "block": block, "p50_us": float(np.percentile(us, 50)),
                       "p95_us": float(np.percentile(us, 95)),
                       "hash": hashlib.sha256(levels.tobytes() + spectrum.tobytes()).hexdigest()}
    result["threads"] = winstats.threads()
    return result


def measure_overlay(args: argparse.Namespace, src: Path, stem: Path) -> dict:
    json_out = stem.with_suffix(".child.json")
    command = [sys.executable, str(Path(__file__)), "overlay-child", "--repeats", str(args.repeats), "--json-out", str(json_out)]
    with open(stem.with_suffix(".log"), "w", encoding="utf-8") as log:
        code = subprocess.run(command, env=child_env(src), cwd=src.parent, stdout=log, stderr=subprocess.STDOUT).returncode
    row = json.loads(json_out.read_text(encoding="utf-8")) if json_out.is_file() else {"kind": "overlay", "render": {}}
    json_out.unlink(missing_ok=True)
    row["exit_code"] = code
    return row


# --- summary --------------------------------------------------------------------------------------------


def percentile(values: list[float], q: float) -> float:
    ordered = sorted(values)
    position = (len(ordered) - 1) * q
    low = int(position)
    high = min(low + 1, len(ordered) - 1)
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def spread(values: list[float]) -> dict:
    return {"n": len(values), "median": statistics.median(values), "min": min(values), "max": max(values),
            "p95": percentile(values, 0.95)} if values else {"n": 0}


def summarize(out: Path) -> dict:
    rows = [json.loads(path.read_text(encoding="utf-8")) for path in sorted(out.glob("*-measured.json"))]
    summary: dict = {}
    for row in rows:
        side = summary.setdefault(row["kind"], {}).setdefault(row["side"], {"rows": [], "failed_runs": 0})
        if row.get("exit_code") or "error" in row:
            side["failed_runs"] += 1
        else:
            side["rows"].append(row)
    for kind, sides in summary.items():
        for name, side in sides.items():
            side_rows = side.pop("rows")
            side["runs"] = len(side_rows)
            side["source_digests"] = sorted({r.get("source_digest", "?") for r in side_rows})
            side["metrics"] = {key: spread(values) for key, values in METRICS[kind](side_rows).items()}
    if "model" in summary:
        summary["model_transcripts"] = transcript_check(out)
    return summary


def model_metrics(rows: list[dict]) -> dict[str, list[float]]:
    # check_model imports onnx_asr, so only the parent may import it: a child must import the app first.
    from check_model import word_error_rate, words

    metrics: dict[str, list[float]] = {}

    def add(key: str, value: float) -> None:
        metrics.setdefault(key, []).append(value)

    for row in rows:
        add("import_s", row["import_s"])
        add("load_s", row["load_s"])
        add("warm_up_s", row["warm_up_s"])
        add("ready_since_start_s", row["phases"]["warm"]["since_start_s"])
        for phase in ("imported", "loaded", "warm", "after_new_length", "after_dictation", "idle_30s"):
            values = row["phases"][phase]
            for key in ("working_set_mb", "private_working_set_mb", "commit_mb", "threads", "gpu_dedicated_mb"):
                if key in values:
                    add(f"{phase}.{key}", values[key])
        last = row["phases"]["idle_30s"]
        add("peak_working_set_mb", last["peak_working_set_mb"])
        add("peak_commit_mb", last["peak_commit_mb"])
        add("cpu_total_s", last["cpu_s"])
        add("cpu_load_and_warm_s", row["phases"]["warm"]["cpu_s"] - row["phases"]["imported"]["cpu_s"])
        for window, value in row["idle_cpu_s"].items():
            add(f"idle_cpu_{window}", value)
        add("errors", sum("error" in r for rows_ in row["sets"].values() for r in rows_))
        add("empty_transcripts", sum(r.get("text", "x").strip() == "" for rows_ in row["sets"].values() for r in rows_))
        for set_name, set_rows in row["sets"].items():
            by_clip: dict[str, list[dict]] = {}
            for r in set_rows:
                by_clip.setdefault(r.get("clip", "all"), []).append(r)
            for clip, clip_rows in by_clip.items():
                # The first call of each clip pays what a size not seen since warm-up costs; the rest show
                # the steady state. In same_length the first call is only the priming one.
                first, rest = clip_rows[0], clip_rows[1:]
                if set_name == "new_length" and "error" not in first:
                    add(f"{set_name}.{clip}.first_ms", first["ms"])
                for r in rest if set_name in ("new_length", "same_length") else clip_rows:
                    if "error" not in r:
                        add(f"{set_name}.{clip}.ms", r["ms"])
        for clip in ("en", "pt"):
            want = words((FIXTURES / f"{clip}.txt").read_text(encoding="utf-8"))
            for r in row["sets"]["new_length"]:
                if r["clip"] == clip and "text" in r:
                    add(f"wer.{clip}", word_error_rate(want, words(r["text"])))
    return metrics


def transcripts(out: Path) -> dict[tuple[str, int], dict[str, set[str]]]:
    """The texts each side produced for call i of each set, across its processes."""
    texts: dict[tuple[str, int], dict[str, set[str]]] = {}
    for path in sorted(out.glob("model-*-measured.json")):
        row = json.loads(path.read_text(encoding="utf-8"))
        for set_name, set_rows in row.get("sets", {}).items():
            for i, r in enumerate(set_rows):
                texts.setdefault((set_name, i), {}).setdefault(row["side"], set()).add(r.get("text", "<error>"))
    return texts


def transcript_check(out: Path) -> dict:
    """Whether every call produced the same text in every process, per side and across sides. Call i of a
    set gets the same input in every process, so differing text means differing numerics, not input."""
    texts = transcripts(out)
    unstable = {name: 0 for sides in texts.values() for name in sides}
    differ = []
    for (set_name, i), sides in texts.items():
        for name, seen in sides.items():
            unstable[name] += len(seen) > 1
        if len({frozenset(seen) for seen in sides.values()}) > 1:
            differ.append({"set": set_name, "call": i, **{name: sorted(seen) for name, seen in sides.items()}})
    return {"calls": len(texts), "calls_varying_within_side": unstable, "calls_differing_between_sides": len(differ),
            "differences": differ}


METRICS = {
    "model": model_metrics,
    "settings": lambda rows: {key: [r[key] for r in rows if key in r] for key in
                              ("window_s", "working_set_mb", "private_working_set_mb", "commit_mb", "threads", "modules")},
    "overlay": lambda rows: {f"{key}.{stat}": [r["render"][key][stat] for r in rows] for key in rows[0]["render"]
                             for stat in ("p50_ms", "p95_ms")} | {"meter.p50_us": [r["meter"]["p50_us"] for r in rows],
                                                                 "threads": [r["threads"] for r in rows if "threads" in r]},
}


def print_summary(summary: dict) -> None:
    for kind, sides in summary.items():
        if kind not in METRICS:
            continue
        names = list(sides)
        print(f"\n{kind}: " + ", ".join(f"{n} ({sides[n]['runs']} runs, {sides[n]['failed_runs']} failed, "
                                         f"src {','.join(sides[n]['source_digests'])})" for n in names))
        keys = list(dict.fromkeys(k for n in names for k in sides[n]["metrics"]))
        print(f"{'metric':44} " + " ".join(f"{n + ' median [min-max]':>32}" for n in names))
        for key in keys:
            cells = []
            for n in names:
                m = sides[n]["metrics"].get(key, {"n": 0})
                cells.append(f"{m['median']:>12.2f} [{m['min']:.2f}-{m['max']:.2f}]" if m["n"] else f"{'-':>32}")
            print(f"{key:44} " + " ".join(f"{c:>32}" for c in cells))
    if check := summary.get("model_transcripts"):
        print(f"\ntranscripts: {check['calls']} call positions; varying within a side: {check['calls_varying_within_side']}; "
              f"differing between sides: {check['calls_differing_between_sides']}")


def compare(before: Path, after: Path, before_side: str | None, after_side: str | None) -> str:
    """A Markdown table per kind: each metric's median before and after, with the change."""

    def pick(out: Path, side: str | None) -> tuple[str, dict]:
        summary = summarize(out)
        kinds = [kind for kind in summary if kind in METRICS]
        if len(kinds) != 1:
            raise SystemExit(f"{out} holds {kinds}; compare one kind at a time")
        sides = summary[kinds[0]]
        if side is None:
            if len(sides) != 1:
                raise SystemExit(f"{out} holds sides {list(sides)}; name one")
            side = next(iter(sides))
        return side, {"kind": kinds[0], **sides[side]}

    def cell(m: dict) -> str:
        return f"{m['median']:.2f} [{m['min']:.2f}-{m['max']:.2f}]" if m.get("n") else "-"

    (b_name, b), (a_name, a) = pick(before, before_side), pick(after, after_side)
    if b["kind"] != a["kind"]:
        raise SystemExit(f"cannot compare {b['kind']} with {a['kind']}")
    lines = [f"| metric | before: {b_name}, n={b['runs']} | after: {a_name}, n={a['runs']} | change | change % |",
             "|---|---:|---:|---:|---:|"]
    for key in dict.fromkeys([*b["metrics"], *a["metrics"]]):
        mb, ma = b["metrics"].get(key, {}), a["metrics"].get(key, {})
        if mb.get("n") and ma.get("n"):
            delta = ma["median"] - mb["median"]
            percent = f"{100 * delta / mb['median']:+.1f}%" if mb["median"] else "-"
            change = f"{delta:+.2f}"
        else:
            change = percent = "-"
        lines.append(f"| {key} | {cell(mb)} | {cell(ma)} | {change} | {percent} |")
    lines.append("")
    lines.append(f"Failed runs: before {b['failed_runs']}, after {a['failed_runs']}. "
                 f"Sources: before {', '.join(b['source_digests'])}, after {', '.join(a['source_digests'])}.")
    if b["kind"] == "model":
        texts_b, texts_a = transcripts(before), transcripts(after)
        positions = set(texts_b) & set(texts_a)
        differ = sorted(p for p in positions if texts_b[p].get(b_name) != texts_a[p].get(a_name))
        lines.append(f"Transcripts: {len(positions)} call positions compared, {len(differ)} differ"
                     + (f": {differ}" if differ else "."))
    return "\n".join(lines)


# --- entry ----------------------------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("model", "settings", "overlay"):
        command = commands.add_parser(name)
        command.add_argument("--side", action="append", help="NAME=SRC, a checkout's src folder; repeat to alternate")
        command.add_argument("--runs", type=int, default=5, help="measured processes per side")
        command.add_argument("--warmup", type=int, default=1, help="discarded processes per side before measuring")
        command.add_argument("--out", required=True)
    commands.choices["model"].add_argument("--model-dir", required=True)
    commands.choices["model"].add_argument("--provider", choices=("cuda", "cpu"), default="cuda")
    commands.choices["model"].add_argument("--reps", type=int, default=5)
    commands.choices["settings"].add_argument("--settle", type=float, default=1.5, help="seconds after the window shows")
    commands.choices["overlay"].add_argument("--repeats", type=int, default=3)
    child = commands.add_parser("model-child")
    child.add_argument("--model-dir", required=True)
    child.add_argument("--provider", required=True)
    child.add_argument("--reps", type=int, required=True)
    child.add_argument("--json-out", required=True)
    overlay_child = commands.add_parser("overlay-child")
    overlay_child.add_argument("--repeats", type=int, required=True)
    overlay_child.add_argument("--json-out", required=True)
    summary = commands.add_parser("summary")
    summary.add_argument("out")
    compared = commands.add_parser("compare", help="a Markdown table of medians before and after")
    compared.add_argument("before")
    compared.add_argument("after")
    compared.add_argument("--before-side")
    compared.add_argument("--after-side")
    args = parser.parse_args()

    if args.command == "model-child":
        Path(args.json_out).write_text(json.dumps(run_model(args), ensure_ascii=False), encoding="utf-8")
    elif args.command == "overlay-child":
        Path(args.json_out).write_text(json.dumps(run_overlay(args)), encoding="utf-8")
    elif args.command == "compare":
        print(compare(Path(args.before), Path(args.after), args.before_side, args.after_side))
    elif args.command == "summary":
        result = summarize(Path(args.out))
        (Path(args.out) / "summary.json").write_text(json.dumps(result, indent=1), encoding="utf-8")
        print_summary(result)
    else:
        measure = {"model": measure_model, "settings": measure_settings, "overlay": measure_overlay}[args.command]
        run_children(args, args.command, measure)
        result = summarize(Path(args.out))
        (Path(args.out) / "summary.json").write_text(json.dumps(result, indent=1), encoding="utf-8")
        print_summary(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
