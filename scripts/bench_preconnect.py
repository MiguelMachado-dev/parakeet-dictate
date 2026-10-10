"""Key-up-to-paste for polished dictations after an idle gap, with and without pre-connecting at key-down.

Runs the app's controller headless: events are posted directly, no keyboard hook is
installed, and nothing is pasted. Needs ZAI_API_KEY; sends the en fixture to Z.ai.
"""

import argparse
import logging
import statistics
import sys
import time

from e2e_check import MIC_RATE, FixtureRecorder, load_fixture
from parakeet_dictate.app import Cues, Dictation
from parakeet_dictate.config import Config, load_api_key
from parakeet_dictate.polish import GlmClient
from parakeet_dictate.reducer import HotkeyDown, HotkeyUp
from parakeet_dictate.transcriber import load_transcriber


class ConnectLog(logging.Handler):
    """Records when httpcore2 starts and finishes opening a connection, and on which thread."""

    def __init__(self) -> None:
        super().__init__(logging.DEBUG)
        self.events: list[tuple[float, str, str]] = []

    def emit(self, record: logging.LogRecord) -> None:
        message = record.getMessage()
        if message.startswith(("connect_tcp.started", "start_tls.complete")):
            self.events.append((time.perf_counter(), message.split(" ", 1)[0], record.threadName))


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser()
    parser.add_argument("--samples", type=int, default=5, help="per condition")
    parser.add_argument("--idle", type=float, default=35.0, help="seconds between dictations; Z.ai drops idle connections after 22 to 26 s")
    parser.add_argument("--record", type=float, default=2.0, help="seconds between key-down and key-up")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="  %(threadName)s %(message)s", stream=sys.stdout)
    for name in ("parakeet_dictate", "httpx2"):
        logging.getLogger(name).setLevel(logging.WARNING)
    logging.getLogger("parakeet_dictate.polish").setLevel(logging.INFO)
    connects = ConnectLog()
    logging.getLogger("httpcore2.connection").addHandler(connects)
    logging.getLogger("httpcore2.connection").setLevel(logging.DEBUG)
    logging.getLogger("httpcore2.connection").propagate = False

    key = load_api_key()
    if key is None:
        print("ZAI_API_KEY not found in the environment or .env")
        return 2
    config = Config()
    transcriber = load_transcriber(config.model_dir, "cuda")
    transcriber.warm_up(MIC_RATE)
    recorder = FixtureRecorder()
    recorder.audio = load_fixture("en")[0]
    glm = GlmClient(config.postprocess, key)
    pastes: list[float] = []
    dictation = Dictation(
        config, recorder=recorder, transcriber=transcriber, paste=lambda text: pastes.append(time.perf_counter()),
        cues=Cues(enabled=False), glm=glm,
    )
    # Only the controller thread: a keyboard hook would see the user's keys during a long run.
    dictation._controller.start()
    preconnect = glm.preconnect
    results: dict[str, list[float]] = {"cold": [], "pre-connect": []}
    try:
        for i in range(2 * args.samples):
            condition = "pre-connect" if i % 2 else "cold"
            glm.preconnect = preconnect if condition == "pre-connect" else lambda: None
            time.sleep(args.idle)
            dictation.post(HotkeyDown(time.perf_counter()))
            time.sleep(args.record)
            released = time.perf_counter()
            dictation.post(HotkeyUp(released))
            count = len(pastes)
            while len(pastes) == count:
                time.sleep(0.005)
            ms = (pastes[-1] - released) * 1000
            results[condition].append(ms)
            opened = [(t, step, thread) for t, step, thread in connects.events if t > released - args.record]
            setup = [f"{step} {(t - released) * 1000:+.0f} ms on {thread}" for t, step, thread in opened]
            print(f"[{condition:11}] key-up to paste {ms:5.0f} ms; connection steps relative to key-up: {setup or 'none'}")
    finally:
        dictation.stop()
        glm.close()

    for condition, values in results.items():
        print(f"{condition:11} n={len(values)} median {statistics.median(values):.0f} ms, "
              f"min {min(values):.0f}, max {max(values):.0f}: {[round(v) for v in values]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
