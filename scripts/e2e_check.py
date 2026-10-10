import argparse
import ctypes
import dataclasses
import json
import logging
import subprocess
import sys
import threading
import time
import wave
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from ctypes import wintypes
from pathlib import Path

import numpy as np
import pynput.keyboard
from PIL import ImageGrab

from check_model import word_error_rate, words
from parakeet_dictate import inject
from parakeet_dictate.app import Cues, Dictation, acquire_single_instance
from parakeet_dictate.config import Config, load_api_key
from parakeet_dictate.meter import LevelMeter
from parakeet_dictate.overlay import Overlay
from parakeet_dictate.overlay_frame import Phase
from parakeet_dictate.polish import GlmClient
from parakeet_dictate.recorder import Audio
from parakeet_dictate.reducer import AwaitingRelease, Idle, Polishing, Recording, State, Transition
from parakeet_dictate.transcriber import load_transcriber, resample

SCRIPTS = Path(__file__).resolve().parent
FIXTURES = SCRIPTS.parent / "tests" / "fixtures"
MIC_RATE = 44_100
SENTINEL = "parakeet-e2e clipboard sentinel ✓"
MAX_WER = 0.15
HOTKEY = inject.VK_RCONTROL
VK_C = 0x43
UNREACHABLE_ENDPOINT = "https://127.0.0.1:9/api/coding/paas/v4/chat/completions"
TINY_TIMEOUT_S = 0.3
GLM_WAIT_S = 15.0
FEED_CHUNK_S = 0.02
# How soon the second key of a Right Ctrl shortcut arrives in the shortcut flow.
SHORTCUT_GAP_S = 0.15
WATCH_INTERVAL_S = 0.005
GWL_EXSTYLE = -20
# WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST
OVERLAY_EXSTYLE = 0x00080000 | 0x00000020 | 0x08000000 | 0x00000080 | 0x00000008
BOTTOM_MARGIN = 48
DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4

user32 = ctypes.WinDLL("user32", use_last_error=True)
shcore = ctypes.WinDLL("shcore")
user32.GetForegroundWindow.restype = wintypes.HWND
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user32.WindowFromPoint.argtypes = [wintypes.POINT]
user32.WindowFromPoint.restype = wintypes.HWND
user32.GetWindowLongW.argtypes = [wintypes.HWND, ctypes.c_int]
user32.SetThreadDpiAwarenessContext.argtypes = [ctypes.c_void_p]
user32.SetThreadDpiAwarenessContext.restype = ctypes.c_void_p
user32.EnumDisplayMonitors.argtypes = [wintypes.HDC, ctypes.POINTER(wintypes.RECT), ctypes.c_void_p, wintypes.LPARAM]
user32.GetMonitorInfoW.argtypes = [wintypes.HMONITOR, ctypes.c_void_p]
shcore.GetDpiForMonitor.argtypes = [wintypes.HMONITOR, ctypes.c_int, ctypes.POINTER(wintypes.UINT), ctypes.POINTER(wintypes.UINT)]


class Abort(Exception):
    pass


class FixtureRecorder:
    """Plays the fixture into the level meter in real time, as the microphone would, and hands
    the whole fixture to the transcriber on stop."""

    device_name = "fixture"

    def __init__(self) -> None:
        self.audio: Audio | None = None
        self.discards = 0
        self.meter = LevelMeter(MIC_RATE)
        self._halt = threading.Event()
        self._feeder: threading.Thread | None = None

    def start(self) -> None:
        assert self.audio is not None
        self._stop_feeding()
        self.meter = LevelMeter(self.audio.rate)
        self._halt = threading.Event()
        self._feeder = threading.Thread(target=self._feed, args=(self.audio, self.meter, self._halt), daemon=True)
        self._feeder.start()

    def stop(self) -> Audio:
        assert self.audio is not None
        self._stop_feeding()
        return self.audio

    def discard(self) -> None:
        self._stop_feeding()
        self.discards += 1

    def _stop_feeding(self) -> None:
        self._halt.set()
        if self._feeder is not None:
            self._feeder.join()
            self._feeder = None

    @staticmethod
    def _feed(audio: Audio, meter: LevelMeter, halt: threading.Event) -> None:
        chunk = round(audio.rate * FEED_CHUNK_S)
        started = time.perf_counter()
        for i, offset in enumerate(range(0, len(audio.samples), chunk), 1):
            if halt.wait(max(0.0, started + i * FEED_CHUNK_S - time.perf_counter())):
                return
            meter.feed(audio.samples[offset : offset + chunk])


class ErrorLog(logging.Handler):
    def __init__(self) -> None:
        super().__init__(logging.ERROR)
        self.records: list[logging.LogRecord] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.records.append(record)


class CountingGlm:
    """GlmClient that records each finished request, so a flow can wait for a reply it expects to be ignored."""

    def __init__(self, glm: GlmClient) -> None:
        self.glm = glm
        self.finished: list[str] = []

    def preconnect(self) -> None:
        self.glm.preconnect()

    def polish(self, raw: str) -> str:
        try:
            text = self.glm.polish(raw)
        except Exception as e:
            self.finished.append(f"failed: {e}")
            raise
        self.finished.append("polished")
        return text

    def wait_for_reply(self, count: int) -> str:
        deadline = time.perf_counter() + GLM_WAIT_S
        while len(self.finished) < count:
            if time.perf_counter() > deadline:
                raise AssertionError(f"GLM request {count} did not finish within {GLM_WAIT_S} s")
            time.sleep(0.02)
        return self.finished[count - 1]


class Target:
    def __init__(self) -> None:
        self.proc = subprocess.Popen(
            [sys.executable, str(SCRIPTS / "e2e_target.py")],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        self.pid = json.loads(self.proc.stdout.readline())["pid"]

    def ask(self, command: str) -> dict:
        self.proc.stdin.write(command + "\n")
        self.proc.stdin.flush()
        return json.loads(self.proc.stdout.readline())

    def text(self) -> str:
        return self.ask("text")["text"]

    def close(self) -> None:
        try:
            self.proc.stdin.write("quit\n")
            self.proc.stdin.flush()
            self.proc.wait(timeout=5)
        except (OSError, subprocess.TimeoutExpired):
            self.proc.kill()
            self.proc.wait()


class Keyboard:
    def __init__(self, target: Target) -> None:
        self.target = target
        self.held: set[int] = set()

    def require_foreground(self) -> None:
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(user32.GetForegroundWindow(), ctypes.byref(pid))
        if pid.value != self.target.pid:
            raise Abort(f"foreground is pid {pid.value}, not the target window (pid {self.target.pid})")

    def press(self, vk: int) -> float:
        self.require_foreground()
        pressed_at = time.perf_counter()
        inject.send_keys([inject.Stroke(vk, up=False)], tag=0)
        self.held.add(vk)
        return pressed_at

    def auto_repeat(self, vk: int, times: int, interval_s: float) -> None:
        for _ in range(times):
            time.sleep(interval_s)
            self.press(vk)

    def release(self, vk: int) -> float:
        self.require_foreground()
        released_at = time.perf_counter()
        inject.send_keys([inject.Stroke(vk, up=True)], tag=0)
        self.held.discard(vk)
        return released_at

    def release_held_keys_unguarded(self) -> None:
        for vk in sorted(self.held):
            inject.send_keys([inject.Stroke(vk, up=True)], tag=0)
        self.held.clear()


class OverlayWatch:
    """Samples the overlay on its own thread, in physical pixels: who has focus, whether the overlay
    is visible, what is under its center, and what it draws."""

    def __init__(self, overlay: Overlay) -> None:
        self.overlay = overlay
        self.samples = 0
        self.focused = 0
        self.hit_tests = 0
        self.hits = 0
        self.rect = wintypes.RECT()
        self.shown: list[list[float]] = []
        self.phases: list[tuple[float, Phase | None]] = []
        self.recording_level_max = 0.0
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, name="overlay-watch", daemon=True)
        self._thread.start()

    def visible_between(self, start: float, end: float) -> bool:
        return any(shown_at <= end and (hidden_at is None or hidden_at >= start) for shown_at, hidden_at in self.shown)

    def hidden_after(self, t: float) -> float | None:
        return next((hidden_at for shown_at, hidden_at in self.shown if shown_at <= t and hidden_at and hidden_at >= t), None)

    def phases_between(self, start: float, end: float) -> set[Phase]:
        return {phase for t, phase in self.phases if start <= t <= end and phase is not None}

    def stop(self) -> None:
        self._stop.set()
        self._thread.join()

    def _run(self) -> None:
        user32.SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        hwnd = self.overlay.hwnd
        rect = wintypes.RECT()
        while not self._stop.wait(WATCH_INTERVAL_S):
            now = time.perf_counter()
            self.samples += 1
            if user32.GetForegroundWindow() == hwnd:
                self.focused += 1
            visible = bool(user32.IsWindowVisible(hwnd))
            if visible and (not self.shown or self.shown[-1][1] is not None):
                self.shown.append([now, None])
            elif not visible and self.shown and self.shown[-1][1] is None:
                self.shown[-1][1] = now
            if visible:
                user32.GetWindowRect(hwnd, ctypes.byref(rect))
                self.rect = wintypes.RECT(rect.left, rect.top, rect.right, rect.bottom)
                self.hit_tests += 1
                if user32.WindowFromPoint(wintypes.POINT((rect.left + rect.right) // 2, (rect.top + rect.bottom) // 2)) == hwnd:
                    self.hits += 1
            frame = self.overlay.last_frame
            phase = frame.phase if frame is not None else None
            if not self.phases or self.phases[-1][1] is not phase:
                self.phases.append((now, phase))
            if frame is not None and phase is Phase.RECORDING:
                self.recording_level_max = max(self.recording_level_max, float(frame.levels.max()))


def work_areas() -> list[tuple[wintypes.RECT, int]]:
    """Each monitor's work area and DPI, in physical pixels."""
    from parakeet_dictate.overlay import MONITORINFO

    previous = user32.SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
    found: list[tuple[wintypes.RECT, int]] = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HMONITOR, wintypes.HDC, ctypes.POINTER(wintypes.RECT), wintypes.LPARAM)
    def visit(monitor: int, dc: int, rect: object, data: int) -> bool:
        info = MONITORINFO(cbSize=ctypes.sizeof(MONITORINFO))
        user32.GetMonitorInfoW(monitor, ctypes.byref(info))
        dpi, _ = wintypes.UINT(), wintypes.UINT()
        shcore.GetDpiForMonitor(monitor, 0, ctypes.byref(dpi), ctypes.byref(_))
        found.append((info.rcWork, dpi.value))
        return True

    try:
        user32.EnumDisplayMonitors(None, None, ctypes.cast(visit, ctypes.c_void_p), 0)
    finally:
        user32.SetThreadDpiAwarenessContext(previous)
    return found


def load_fixture(name: str) -> tuple[Audio, str]:
    with wave.open(str(FIXTURES / f"{name}.wav")) as f:
        samples = np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768
    expected = (FIXTURES / f"{name}.txt").read_text(encoding="utf-8").strip()
    return Audio(resample(samples, 16_000, MIC_RATE), MIC_RATE), expected


def wait_until[T](probe: Callable[[], T | None], what: str, timeout_s: float = GLM_WAIT_S) -> tuple[T, float]:
    deadline = time.perf_counter() + timeout_s
    while time.perf_counter() < deadline:
        if found := probe():
            return found, time.perf_counter()
        time.sleep(0.005)
    raise AssertionError(f"timed out waiting for {what}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--screenshots", type=Path, help="save desktop grabs of the overlay while it records here")
    args = parser.parse_args()
    sys.stdout.reconfigure(encoding="utf-8")
    logging.basicConfig(level=logging.INFO, format="  %(threadName)s %(message)s", stream=sys.stdout)
    logging.getLogger("httpx2").setLevel(logging.WARNING)
    app_errors = ErrorLog()
    logging.getLogger("parakeet_dictate").addHandler(app_errors)
    if acquire_single_instance() is None:
        print("parakeet-dictate is running; stop it first (parakeet-dictate --quit)")
        return 2
    key = load_api_key()
    if key is None:
        print("ZAI_API_KEY not found in the environment or .env; the post-processing flows need it")
        return 2

    config = Config()
    transcriber = load_transcriber(config.model_dir, "cuda")
    transcriber.warm_up(MIC_RATE)
    print(f"provider: {transcriber.provider}")
    if transcriber.provider != "CUDAExecutionProvider":
        return 1

    glm = CountingGlm(GlmClient(config.postprocess, key))
    # The unreachable client never needs the real key.
    unreachable = GlmClient(dataclasses.replace(config.postprocess, endpoint=UNREACHABLE_ENDPOINT), "unused")
    tiny_timeout = dataclasses.replace(config, postprocess=dataclasses.replace(config.postprocess, timeout_s=TINY_TIMEOUT_S))
    too_slow = GlmClient(tiny_timeout.postprocess, key)

    fixtures = {name: load_fixture(name) for name in ("en", "pt")}
    recorder = FixtureRecorder()
    states: list[tuple[float, State]] = []
    pastes: list[float] = []
    latencies: dict[str, float] = {}
    original_clipboard = inject.get_text()
    overlay = Overlay(lambda: recorder.meter.snapshot())
    watch = OverlayWatch(overlay)
    target = Target()
    keyboard = Keyboard(target)

    def guarded_paste(text: str) -> None:
        keyboard.require_foreground()
        pastes.append(time.perf_counter())
        inject.paste(text)

    def on_transition(transition: Transition) -> None:
        states.append((time.perf_counter(), transition.state))
        overlay.notify(transition)

    @contextmanager
    def dictating(
        glm_client: GlmClient | CountingGlm | None, enabled: bool = True, app_config: Config = config
    ) -> Iterator[None]:
        dictation = Dictation(
            app_config,
            recorder=recorder,
            transcriber=transcriber,
            paste=guarded_paste,
            cues=Cues(enabled=False),
            on_transition=on_transition,
            glm=glm_client,
            polish_enabled=lambda: enabled,
        )
        dictation.start()
        try:
            yield
        finally:
            keyboard.release_held_keys_unguarded()
            dictation.stop()

    # Low-level hooks run newest first, so this one, installed before the app's, only sees
    # keys the app let through.
    escapes_leaked: list[int] = []

    def witness(msg: int, data: object) -> bool:
        if data.vkCode == inject.VK_ESCAPE:
            escapes_leaked.append(msg)
            witness_hook.suppress_event()
        return False

    witness_hook = pynput.keyboard.Listener(win32_event_filter=witness)
    failures: list[str] = []

    def check(ok: bool, message: str) -> None:
        print(f"  {'ok  ' if ok else 'FAIL'} {message}")
        if not ok:
            failures.append(message)

    def polishing_after(t: float) -> Polishing | None:
        return next((s for ts, s in states if ts >= t and isinstance(s, Polishing)), None)

    def prepare(name: str) -> None:
        target.ask("clear")
        inject.set_text(SENTINEL)
        recorder.audio = fixtures[name][0]

    def hold(name: str, while_recording: Callable[[], None] = lambda: None) -> float:
        prepare(name)
        keyboard.press(HOTKEY)
        keyboard.auto_repeat(HOTKEY, times=4, interval_s=0.2)
        while_recording()
        time.sleep(0.2)
        return keyboard.release(HOTKEY)

    def screenshot(name: str) -> None:
        if args.screenshots is None:
            return
        r = watch.rect
        box = (r.left - 200, r.top - 80, r.right + 200, r.bottom + 120)
        path = args.screenshots / f"{time.strftime('%Y%m%d-%H%M%S')}-{name}.png"
        args.screenshots.mkdir(parents=True, exist_ok=True)
        ImageGrab.grab(box, include_layered_windows=True, all_screens=True).save(path)
        print(f"  saved {path} (overlay at {r.left},{r.top} to {r.right},{r.bottom})")

    def observe_recording(name: str) -> wintypes.RECT:
        frame = overlay.last_frame
        showing = frame is not None and frame.phase is Phase.RECORDING and bool(user32.IsWindowVisible(overlay.hwnd))
        check(showing, "overlay visible mid-recording, drawing RECORDING")
        screenshot(name)
        return watch.rect

    def check_bottom_center(rect: wintypes.RECT, work: wintypes.RECT, dpi: int, where: str) -> None:
        center_off = (rect.left + rect.right) / 2 - (work.left + work.right) / 2
        margin = work.bottom - rect.bottom
        check(
            abs(center_off) <= 1 and abs(margin - BOTTOM_MARGIN * dpi / 96) <= 1 and work.left <= rect.left and rect.right <= work.right,
            f"overlay at the bottom center of the {where} work area ({rect.left},{rect.top} to {rect.right},{rect.bottom}; "
            f"work area {work.left},{work.top} to {work.right},{work.bottom}; {margin} px above its bottom)",
        )

    def landed(name: str, flow: str, released: float) -> str:
        text, seen_at = wait_until(lambda: target.text(), "text in the target window")
        time.sleep(inject.CLIPBOARD_RESTORE_DELAY_S + 0.3)
        wer = word_error_rate(words(fixtures[name][1]), words(text))
        paste_ms = (pastes[-1] - released) * 1000
        latencies[f"{flow} {name}"] = paste_ms
        print(f"[{flow} {name}] got: {text!r}")
        print(f"  key-up to Ctrl+V sent {paste_ms:.0f} ms, to text visible in target {(seen_at - released) * 1000:.0f} ms")
        check(wer <= MAX_WER, f"WER {wer:.2f} <= {MAX_WER}")
        check(text.endswith(" ") and not text.endswith("  "), "one trailing space appended")
        check(inject.get_text() == SENTINEL, "clipboard restored to the sentinel")
        return text

    def work_area_at(x: int, y: int) -> tuple[wintypes.RECT, int] | None:
        return next(((w, dpi) for w, dpi in work_areas() if w.left <= x < w.right and w.top <= y < w.bottom), None)

    witness_hook.start()
    witness_hook.wait()
    try:
        home = target.ask("focus")
        time.sleep(0.3)
        keyboard.require_foreground()
        style = user32.GetWindowLongW(overlay.hwnd, GWL_EXSTYLE)
        check(style & OVERLAY_EXSTYLE == OVERLAY_EXSTYLE, f"overlay is layered, transparent, no-activate, tool window, topmost (0x{style & 0xFFFFFFFF:08x})")

        with dictating(None):
            for name in ("en", "pt"):
                rects: list[wintypes.RECT] = []
                landed(name, "hold", hold(name, lambda: rects.append(observe_recording(f"hold-{name}"))))
                check_bottom_center(rects[-1], *work_area_at(*home["center"]), "target's")

            for name in ("en", "pt"):
                prepare(name)
                keyboard.press(HOTKEY)
                time.sleep(0.08)
                keyboard.release(HOTKEY)
                time.sleep(1.0)
                check(target.text() == "", "tap left recording on, nothing pasted yet")
                pressed = keyboard.press(HOTKEY)
                time.sleep(0.8)
                check(target.text() == "", "nothing pasted while the hotkey is still down")
                released = keyboard.release(HOTKEY)
                deferred = any(isinstance(s, AwaitingRelease) and pressed < t < released for t, s in states)
                check(deferred, "transcript was ready before key-up and waited for it")
                landed(name, "toggle", released)

            prepare("en")
            discards, leaked = recorder.discards, len(escapes_leaked)
            keyboard.press(HOTKEY)
            time.sleep(0.6)
            keyboard.press(inject.VK_ESCAPE)
            keyboard.release(inject.VK_ESCAPE)
            time.sleep(0.2)
            keyboard.release(HOTKEY)
            time.sleep(1.5)
            keyboard.require_foreground()
            print("[esc cancel while holding]")
            check(target.text() == "", "Esc cancelled: nothing pasted")
            check(recorder.discards > discards, "recording discarded")
            check(inject.get_text() == SENTINEL, "clipboard untouched")
            check(len(escapes_leaked) == leaked, "Esc swallowed by the app, so Ctrl+Esc never reached Windows")

            print(f"[shortcut: Right Ctrl, then C {SHORTCUT_GAP_S * 1000:.0f} ms later]")
            prepare("en")
            pressed = keyboard.press(HOTKEY)
            time.sleep(SHORTCUT_GAP_S)
            keyboard.press(VK_C)
            keyboard.release(VK_C)
            keyboard.release(HOTKEY)
            time.sleep(1.0)
            discarded = any(isinstance(s, Idle) and s.key_down and t > pressed for t, s in states)
            check(discarded, "the shortcut discarded the recording")
            check(not watch.visible_between(pressed, time.perf_counter()), "overlay never became visible")
            check(target.text() == "" and inject.get_text() == SENTINEL, "nothing pasted, clipboard untouched")

            print("[shortcut after the overlay appeared]")
            prepare("en")
            pressed = keyboard.press(HOTKEY)
            wait_until(lambda: user32.IsWindowVisible(overlay.hwnd), "the overlay to appear", 2.0)
            time.sleep(0.2)
            second_key = keyboard.press(VK_C)
            keyboard.release(VK_C)
            keyboard.release(HOTKEY)
            time.sleep(1.0)
            hidden_at = watch.hidden_after(second_key)
            check(hidden_at is not None and hidden_at - second_key < 0.05,
                  f"overlay hid {(hidden_at - second_key) * 1000 if hidden_at else float('nan'):.0f} ms after the second key")
            check(not watch.visible_between(hidden_at or second_key, time.perf_counter()), "and stayed hidden")
            beats = watch.phases_between(second_key, time.perf_counter()) & {Phase.DONE, Phase.CANCELLED, Phase.FAILED}
            check(not beats, f"no beat played ({sorted(p.value for p in beats)})")

            left = next(((w, dpi) for w, dpi in work_areas() if w.right <= 0), None)
            if left is None:
                print("[hold on a monitor left of the primary] skipped: no such monitor")
            else:
                work, dpi = left
                target.ask(f"move {work.left + 400} {work.top + 200}")
                moved = target.ask("focus")
                time.sleep(0.3)
                check(work_area_at(*moved["center"])[0].left == work.left, f"target moved to the left monitor (center {moved['center']})")
                rects = []
                landed("en", "hold on the left monitor", hold("en", lambda: rects.append(observe_recording("hold-left-monitor"))))
                check_bottom_center(rects[-1], work, dpi, "left monitor's")
                target.ask(f"move {home['origin'][0]} {home['origin'][1]}")
                target.ask("focus")
                time.sleep(0.3)
                keyboard.require_foreground()

        with dictating(glm, enabled=False):
            requests = len(glm.finished)
            released = hold("en")
            landed("en", "tray toggle off", released)
            check(polishing_after(released) is None and len(glm.finished) == requests, "GLM not called")

        with dictating(glm):
            for name in ("en", "pt"):
                released = hold(name)
                landed(name, "polished hold", released)
                check(polishing_after(released) is not None and glm.finished[-1] == "polished", "went through GLM")

            prepare("pt")
            keyboard.press(HOTKEY)
            time.sleep(0.08)
            keyboard.release(HOTKEY)
            time.sleep(1.0)
            requests = len(glm.finished)
            pressed = keyboard.press(HOTKEY)
            outcome = glm.wait_for_reply(requests + 1)
            time.sleep(0.3)
            check(target.text() == "", f"GLM {outcome} while the hotkey is down; nothing pasted yet")
            released = keyboard.release(HOTKEY)
            deferred = any(isinstance(s, AwaitingRelease) and pressed < t < released for t, s in states)
            check(deferred and outcome == "polished", "polished text waited for key-up")
            landed("pt", "polished toggle", released)

            leaked, requests = len(escapes_leaked), len(glm.finished)
            released = hold("en")
            polishing, _ = wait_until(lambda: polishing_after(released), "the Polishing state")
            time.sleep(0.2)
            esc_at = keyboard.press(inject.VK_ESCAPE)
            # Held past the paste, so the key-up arrives after the state has left Polishing.
            time.sleep(0.3)
            keyboard.release(inject.VK_ESCAPE)
            text, _ = wait_until(lambda: target.text(), "text in the target window")
            latencies["esc skip en (from Esc)"] = (pastes[-1] - esc_at) * 1000
            print(f"[esc while polishing en] got: {text!r}")
            print(f"  Esc to Ctrl+V sent {(pastes[-1] - esc_at) * 1000:.0f} ms")
            check(text == polishing.raw + " ", "raw transcript pasted")
            check(len(glm.finished) == requests, "pasted before GLM replied")
            outcome = glm.wait_for_reply(requests + 1)
            time.sleep(0.5)
            check(target.text() == text, f"late GLM reply ({outcome}) ignored: nothing else pasted")
            check(len(escapes_leaked) == leaked, "Esc down and up swallowed: the target window never received them")
            check(inject.get_text() == SENTINEL, "clipboard restored to the sentinel")

        for flow, client, app_config, name in (
            ("fallback unreachable", unreachable, config, "en"),
            ("fallback timeout", too_slow, tiny_timeout, "pt"),
        ):
            with dictating(client, app_config=app_config):
                released = hold(name)
                text = landed(name, flow, released)
                polishing_at, polishing = next((t, s) for t, s in states if t >= released and isinstance(s, Polishing))
                check(text == polishing.raw + " ", "raw transcript pasted")
                waited_s = pastes[-1] - polishing_at
                print(f"  Polishing to Ctrl+V sent {waited_s * 1000:.0f} ms")
                if client is too_slow:
                    check(waited_s < TINY_TIMEOUT_S + 0.1, f"raw pasted within the {TINY_TIMEOUT_S} s deadline")
    except Abort as e:
        print(f"ABORTED: {e}")
        return 3
    finally:
        keyboard.release_held_keys_unguarded()
        witness_hook.stop()
        watch.stop()
        overlay.close()
        target.close()
        inject.set_text(original_clipboard)
        for client in (glm.glm, unreachable, too_slow):
            client.close()

    print(f"[overlay over the whole run: {watch.samples} samples every {WATCH_INTERVAL_S * 1000:.0f} ms, {overlay.frames} frames drawn]")
    check(watch.focused == 0, f"overlay was never the foreground window ({watch.focused} of {watch.samples} samples)")
    check(watch.hit_tests > 0 and watch.hits == 0,
          f"WindowFromPoint at the overlay's center never returned the overlay ({watch.hits} of {watch.hit_tests} samples while visible)")
    drawn = {phase for _, phase in watch.phases if phase is not None}
    expected = {Phase.RECORDING, Phase.TRANSCRIBING, Phase.POLISHING, Phase.DONE, Phase.CANCELLED}
    check(expected <= drawn, f"overlay drew {sorted(p.value for p in drawn)}")
    check(watch.recording_level_max > 0.5, f"overlay drew live fixture levels while recording (max {watch.recording_level_max:.2f})")
    check(not app_errors.records, f"the app logged no errors ({len(app_errors.records)} logged)")
    print("key-up to Ctrl+V sent:")
    for flow, ms in latencies.items():
        print(f"  {flow:32} {ms:6.0f} ms")
    print(f"provider: {transcriber.provider}; {'PASS' if not failures else f'{len(failures)} FAILED'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
