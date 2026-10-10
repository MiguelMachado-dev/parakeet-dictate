"""End-to-end check of the real pipeline: low-level keyboard hook, reducer, transcriber on CUDA,
and clipboard paste into a separate-process Tk window. Only the microphone is replaced, by a
recorder that returns fixture speech.

The user may be at this PC, so every keystroke is preceded by a check that the target window
is in the foreground, and the run aborts otherwise.
"""

import ctypes
import json
import logging
import subprocess
import sys
import time
import wave
from ctypes import wintypes
from pathlib import Path

import numpy as np
import pynput.keyboard

from check_model import word_error_rate, words
from parakeet_dictate import inject
from parakeet_dictate.app import Cues, Dictation, acquire_single_instance
from parakeet_dictate.config import Config
from parakeet_dictate.recorder import Audio
from parakeet_dictate.reducer import AwaitingRelease, State
from parakeet_dictate.transcriber import load_transcriber, resample

SCRIPTS = Path(__file__).resolve().parent
FIXTURES = SCRIPTS.parent / "tests" / "fixtures"
MIC_RATE = 44_100
SENTINEL = "parakeet-e2e clipboard sentinel ✓"
MAX_WER = 0.15
HOTKEY = inject.VK_RCONTROL

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.GetForegroundWindow.restype = wintypes.HWND
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]


class Abort(Exception):
    pass


class FixtureRecorder:
    device_name = "fixture"

    def __init__(self) -> None:
        self.audio: Audio | None = None
        self.discards = 0

    def start(self) -> None:
        pass

    def stop(self) -> Audio:
        assert self.audio is not None
        return self.audio

    def discard(self) -> None:
        self.discards += 1


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
    """The only path for synthetic keystrokes in this script."""

    def __init__(self, target: Target) -> None:
        self.target = target
        self.held: set[int] = set()

    def require_foreground(self) -> None:
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(user32.GetForegroundWindow(), ctypes.byref(pid))
        if pid.value != self.target.pid:
            raise Abort(f"foreground is pid {pid.value}, not the target window (pid {self.target.pid})")

    def press(self, vk: int) -> None:
        self.require_foreground()
        inject.send_keys([(vk, False)], tag=0)
        self.held.add(vk)

    def release(self, vk: int) -> float:
        self.require_foreground()
        released_at = time.perf_counter()
        inject.send_keys([(vk, True)], tag=0)
        self.held.discard(vk)
        return released_at

    def release_all(self) -> None:
        # A key-up types nothing, and a synthetic key left down would turn the user's next
        # keystrokes into shortcuts, so this one is sent without the foreground check.
        for vk in sorted(self.held):
            inject.send_keys([(vk, True)], tag=0)
        self.held.clear()


def load_fixture(name: str) -> tuple[Audio, str]:
    with wave.open(str(FIXTURES / f"{name}.wav")) as f:
        samples = np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32) / 32768
    expected = (FIXTURES / f"{name}.txt").read_text(encoding="utf-8").strip()
    return Audio(resample(samples, 16_000, MIC_RATE), MIC_RATE), expected


def wait_for_text(target: Target, timeout_s: float = 10.0) -> tuple[str, float]:
    deadline = time.perf_counter() + timeout_s
    while time.perf_counter() < deadline:
        text = target.text()
        if text:
            return text, time.perf_counter()
        time.sleep(0.005)
    raise AssertionError("nothing was pasted into the target window")


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8")
    logging.basicConfig(level=logging.INFO, format="  %(threadName)s %(message)s", stream=sys.stdout)
    if acquire_single_instance() is None:
        print("parakeet-dictate is running; stop it first (parakeet-dictate --quit)")
        return 2

    config = Config()
    transcriber = load_transcriber(config.model_dir, "cuda")
    transcriber.warm_up(MIC_RATE)
    print(f"provider: {transcriber.provider}")
    if transcriber.provider != "CUDAExecutionProvider":
        return 1

    fixtures = {name: load_fixture(name) for name in ("en", "pt")}
    recorder = FixtureRecorder()
    states: list[tuple[float, State]] = []
    pastes: list[float] = []
    original_clipboard = inject.get_text()
    target = Target()
    keyboard = Keyboard(target)

    def guarded_paste(text: str) -> None:
        keyboard.require_foreground()
        pastes.append(time.perf_counter())
        inject.paste(text)

    dictation = Dictation(
        config,
        recorder=recorder,
        transcriber=transcriber,
        paste=guarded_paste,
        cues=Cues(enabled=False),
        on_state=lambda state: states.append((time.perf_counter(), state)),
    )
    # Low-level hooks run newest first, so this one, installed before the app's, only sees
    # keys the app let through. It swallows any Esc it gets: if the app ever failed to,
    # Esc with Ctrl down would open the Start menu on the user's screen.
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

    def landed(name: str, flow: str, released: float) -> None:
        text, seen_at = wait_for_text(target)
        time.sleep(inject.SETTLE_S + 0.3)
        wer = word_error_rate(words(fixtures[name][1]), words(text))
        paste_ms = (pastes[-1] - released) * 1000
        visible_ms = (seen_at - released) * 1000
        print(f"[{flow} {name}] got: {text!r}")
        print(f"  key-up to Ctrl+V sent {paste_ms:.0f} ms, to text visible in target {visible_ms:.0f} ms")
        check(wer <= MAX_WER, f"WER {wer:.2f} <= {MAX_WER}")
        check(text.endswith(" ") and not text.endswith("  "), "one trailing space appended")
        check(inject.get_text() == SENTINEL, "clipboard restored to the sentinel")

    started = False
    witness_hook.start()
    witness_hook.wait()
    try:
        target.ask("focus")
        time.sleep(0.3)
        keyboard.require_foreground()
        dictation.start()
        started = True

        for name in ("en", "pt"):
            target.ask("clear")
            inject.set_text(SENTINEL)
            recorder.audio = fixtures[name][0]
            keyboard.press(HOTKEY)
            for _ in range(4):
                time.sleep(0.2)
                keyboard.press(HOTKEY)  # what keyboard auto-repeat looks like to the hook
            time.sleep(0.2)
            landed(name, "hold", keyboard.release(HOTKEY))

        for name in ("en", "pt"):
            target.ask("clear")
            inject.set_text(SENTINEL)
            recorder.audio = fixtures[name][0]
            keyboard.press(HOTKEY)
            time.sleep(0.08)
            keyboard.release(HOTKEY)
            time.sleep(1.0)
            check(target.text() == "", "tap left recording on, nothing pasted yet")
            keyboard.press(HOTKEY)
            time.sleep(0.8)
            check(target.text() == "", "nothing pasted while the hotkey is still down")
            released = keyboard.release(HOTKEY)
            deferred = any(isinstance(s, AwaitingRelease) and t < released for t, s in states)
            check(deferred, "transcript was ready before key-up and waited for it")
            landed(name, "toggle", released)

        target.ask("clear")
        inject.set_text(SENTINEL)
        discards = recorder.discards
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
        check(not escapes_leaked, "Esc swallowed by the app, so Ctrl+Esc never reached Windows")
    except Abort as e:
        print(f"ABORTED: {e}")
        return 3
    finally:
        keyboard.release_all()
        if started:
            dictation.stop()
        witness_hook.stop()
        target.close()
        inject.set_text(original_clipboard)

    print(f"provider: {transcriber.provider}; {'PASS' if not failures else f'{len(failures)} FAILED'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
