"""How long a key event takes to reach Dictation._on_key, with the overlay hidden and animating.

The renderer shares the GIL with the low-level keyboard hook, and Windows removes a hook that
answers too slowly. This sends F24 to the e2e target window, behind the same foreground guard as
e2e_check, and times each key from just before SendInput to the entry of _on_key. Rounds alternate
hidden and animating; the overlay animates in a hands-free recording fed with fixture audio.
Stop the app first, and do not use the keyboard or mouse while it runs.
"""

import argparse
import importlib
import importlib.util
import random
import sys
import time
from pathlib import Path

import numpy as np

from e2e_check import HOTKEY, Abort, FixtureRecorder, Keyboard, Target, load_fixture, wait_until
from parakeet_dictate import render
from parakeet_dictate.app import WM_KEYDOWN, Cues, Dictation, acquire_single_instance
from parakeet_dictate.config import Config
from parakeet_dictate.overlay import Overlay
from parakeet_dictate.overlay_frame import Phase
from parakeet_dictate.recorder import Audio

VK_F24 = 0x87
ROUNDS = ("hidden", "animating", "hidden", "animating")


class TimedDictation(Dictation):
    def __init__(self, *args: object, **kwargs: object) -> None:
        self.entries: list[float] = []
        super().__init__(*args, **kwargs)

    def _on_key(self, msg: int, data: object) -> bool:
        if data.vkCode == VK_F24 and msg == WM_KEYDOWN:
            self.entries.append(time.perf_counter())
        return super()._on_key(msg, data)


class Silence:
    def transcribe(self, audio: Audio) -> str:
        return ""


def load_renderer(spec: str):
    """module:callable, or path/to/render.py:callable loaded as part of the parakeet_dictate package."""
    target, _, attr = spec.rpartition(":")
    if target.endswith(".py"):
        loader = importlib.util.spec_from_file_location("parakeet_dictate._bench_renderer", Path(target))
        module = importlib.util.module_from_spec(loader)
        loader.loader.exec_module(module)
    else:
        module = importlib.import_module(target)
    return getattr(module, attr)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--renderer", default="parakeet_dictate.render:render")
    parser.add_argument("--keys", type=int, default=150, help="keys per round")
    args = parser.parse_args()
    render.render = load_renderer(args.renderer)
    if acquire_single_instance() is None:
        print("parakeet-dictate is running; stop it first (parakeet-dictate --quit)")
        return 2

    recorder = FixtureRecorder()
    recorder.audio = load_fixture("en")[0]
    overlay = Overlay(lambda: recorder.meter.snapshot())
    target = Target()
    keyboard = Keyboard(target)
    dictation = TimedDictation(
        Config(), recorder=recorder, transcriber=Silence(), paste=lambda text: None, cues=Cues(enabled=False),
        on_transition=overlay.notify,
    )
    results: dict[str, list[float]] = {"hidden": [], "animating": []}
    try:
        target.ask("focus")
        time.sleep(0.3)
        keyboard.require_foreground()
        dictation.start()
        for n, condition in enumerate(ROUNDS, 1):
            if condition == "animating":
                keyboard.press(HOTKEY)
                time.sleep(0.08)
                keyboard.release(HOTKEY)
                wait_until(lambda: overlay.last_frame and overlay.last_frame.presence == 1.0, "the overlay to fade in", 3.0)
            frames = overlay.frames
            dictation.entries.clear()
            sent = []
            for _ in range(args.keys):
                sent.append(keyboard.press(VK_F24))
                keyboard.release(VK_F24)
                time.sleep(random.uniform(0.015, 0.035))
            wait_until(lambda: len(dictation.entries) >= len(sent), "every key to reach the hook", 3.0)
            drawn = overlay.frames - frames
            if condition == "animating":
                check_phase = overlay.last_frame.phase if overlay.last_frame else None
                keyboard.press(HOTKEY)
                keyboard.release(HOTKEY)
                wait_until(lambda: overlay.last_frame is None, "the overlay to hide", 3.0)
                if check_phase is not Phase.RECORDING:
                    raise AssertionError(f"overlay was {check_phase} during an animating round")
            ms = (np.array(dictation.entries[: len(sent)]) - np.array(sent)) * 1000
            results[condition].extend(ms)
            print(f"round {n} {condition:9} {len(ms)} keys, {drawn} frames drawn: "
                  f"p50 {np.percentile(ms, 50):.2f} ms, p95 {np.percentile(ms, 95):.2f} ms, max {ms.max():.2f} ms")
    except Abort as e:
        print(f"ABORTED: {e}")
        return 3
    finally:
        keyboard.release_held_keys_unguarded()
        dictation.stop()
        overlay.close()
        target.close()

    print(f"renderer {args.renderer}")
    for condition, values in results.items():
        ms = np.array(values)
        print(f"  {condition:9} n={len(ms)}  p50 {np.percentile(ms, 50):.2f} ms  p95 {np.percentile(ms, 95):.2f} ms  max {ms.max():.2f} ms")
    growth = np.percentile(results["animating"], 95) - np.percentile(results["hidden"], 95)
    print(f"p95 growth while animating: {growth:+.2f} ms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
