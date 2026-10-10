import ctypes
import dataclasses
import json
import logging
import subprocess
import sys
import time
import wave
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from ctypes import wintypes
from pathlib import Path

import numpy as np
import pynput.keyboard

from check_model import word_error_rate, words
from parakeet_dictate import inject
from parakeet_dictate.app import Cues, Dictation, acquire_single_instance
from parakeet_dictate.config import Config, load_api_key
from parakeet_dictate.polish import GlmClient
from parakeet_dictate.recorder import Audio
from parakeet_dictate.reducer import AwaitingRelease, Polishing, State
from parakeet_dictate.transcriber import load_transcriber, resample

SCRIPTS = Path(__file__).resolve().parent
FIXTURES = SCRIPTS.parent / "tests" / "fixtures"
MIC_RATE = 44_100
SENTINEL = "parakeet-e2e clipboard sentinel ✓"
MAX_WER = 0.15
HOTKEY = inject.VK_RCONTROL
UNREACHABLE_ENDPOINT = "https://127.0.0.1:9/api/coding/paas/v4/chat/completions"
TINY_TIMEOUT_S = 0.3
GLM_WAIT_S = 15.0

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


class CountingGlm:
    """GlmClient that records each finished request, so a flow can wait for a reply it expects to be ignored."""

    def __init__(self, glm: GlmClient) -> None:
        self.glm = glm
        self.finished: list[str] = []

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
    sys.stdout.reconfigure(encoding="utf-8")
    logging.basicConfig(level=logging.INFO, format="  %(threadName)s %(message)s", stream=sys.stdout)
    logging.getLogger("httpx2").setLevel(logging.WARNING)
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
    target = Target()
    keyboard = Keyboard(target)

    def guarded_paste(text: str) -> None:
        keyboard.require_foreground()
        pastes.append(time.perf_counter())
        inject.paste(text)

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
            on_state=lambda state: states.append((time.perf_counter(), state)),
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

    def hold(name: str) -> float:
        prepare(name)
        keyboard.press(HOTKEY)
        keyboard.auto_repeat(HOTKEY, times=4, interval_s=0.2)
        time.sleep(0.2)
        return keyboard.release(HOTKEY)

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

    witness_hook.start()
    witness_hook.wait()
    try:
        target.ask("focus")
        time.sleep(0.3)
        keyboard.require_foreground()

        with dictating(None):
            for name in ("en", "pt"):
                landed(name, "hold", hold(name))

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
        target.close()
        inject.set_text(original_clipboard)
        for client in (glm.glm, unreachable, too_slow):
            client.close()

    print("key-up to Ctrl+V sent:")
    for flow, ms in latencies.items():
        print(f"  {flow:32} {ms:6.0f} ms")
    print(f"provider: {transcriber.provider}; {'PASS' if not failures else f'{len(failures)} FAILED'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
