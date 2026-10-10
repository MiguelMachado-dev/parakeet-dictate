import argparse
import ctypes
import itertools
import logging
import math
import os
import queue
import struct
import sys
import tempfile
import threading
import time
import wave
import winsound
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from ctypes import wintypes
from logging.handlers import RotatingFileHandler
from pathlib import Path

import pystray
from PIL import Image, ImageDraw
from pynput import keyboard

from . import inject
from .config import LOG_PATH, Config, ConfigError, load_config
from .recorder import Audio, Recorder, default_input_rate
from .reducer import (
    AwaitingRelease,
    Cancel,
    Cue,
    DiscardRecording,
    Effect,
    Event,
    HotkeyDown,
    HotkeyUp,
    Idle,
    OtherKeyDown,
    Paste,
    PlaySound,
    Recording,
    RecordingFailed,
    StartRecording,
    State,
    StopAndTranscribe,
    Transcribing,
    TranscriptReady,
    step,
)
from .transcriber import Transcriber, load_transcriber

log = logging.getLogger("parakeet_dictate")

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
user32.GetAsyncKeyState.argtypes = [ctypes.c_int]
user32.GetAsyncKeyState.restype = ctypes.c_short
kernel32.CreateMutexW.argtypes = [wintypes.LPVOID, wintypes.BOOL, wintypes.LPCWSTR]
kernel32.CreateMutexW.restype = wintypes.HANDLE
kernel32.CreateEventW.argtypes = [wintypes.LPVOID, wintypes.BOOL, wintypes.BOOL, wintypes.LPCWSTR]
kernel32.CreateEventW.restype = wintypes.HANDLE
kernel32.OpenEventW.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.LPCWSTR]
kernel32.OpenEventW.restype = wintypes.HANDLE
kernel32.SetEvent.argtypes = [wintypes.HANDLE]
kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
kernel32.WaitForSingleObject.restype = wintypes.DWORD
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]

MUTEX_NAME = "Local\\parakeet-dictate"
QUIT_EVENT_NAME = "Local\\parakeet-dictate-quit"
ERROR_ALREADY_EXISTS = 183
EVENT_MODIFY_STATE = 0x0002
INFINITE = 0xFFFFFFFF
WAIT_OBJECT_0 = 0
WM_KEYDOWN, WM_SYSKEYDOWN = 0x0100, 0x0104
RELEASE_POLL_S = 0.05


class Dictation:
    def __init__(
        self,
        config: Config,
        *,
        recorder: Recorder,
        transcriber: Transcriber,
        paste: Callable[[str], None] = inject.paste,
        cues: "Cues",
        on_state: Callable[[State], None] = lambda state: None,
    ) -> None:
        self._config = config
        self._recorder = recorder
        self._transcriber = transcriber
        self._paste = paste
        self._cues = cues
        self._on_state = on_state
        self._events: queue.Queue[Event | None] = queue.Queue()
        self._state: State = Idle()
        self._released_at = 0.0
        self._worker = ThreadPoolExecutor(max_workers=1, thread_name_prefix="transcribe")
        self._request_ids = itertools.count(1)
        self._controller = threading.Thread(target=self._run, name="controller", daemon=True)
        self._listener = keyboard.Listener(win32_event_filter=self._on_key)
        self._hook_hotkey_held = False

    def start(self) -> None:
        self._controller.start()
        self._listener.start()
        self._listener.wait()

    def stop(self) -> None:
        self._listener.stop()
        self._events.put(None)
        self._controller.join(timeout=10)
        self._worker.shutdown(wait=True, cancel_futures=True)
        self._recorder.discard()

    def post(self, event: Event) -> None:
        self._events.put(event)

    def _on_key(self, msg: int, data: object) -> bool:
        # Runs inside the low-level hook: post and return. Windows drops slow hooks,
        # and an exception here would stop the pynput listener.
        swallow = False
        try:
            if data.dwExtraInfo != inject.INJECTED_TAG:
                down = msg in (WM_KEYDOWN, WM_SYSKEYDOWN)
                if data.vkCode == self._config.hotkey_vk:
                    self._hook_hotkey_held = down
                    self.post(HotkeyDown(time.perf_counter()) if down else HotkeyUp(time.perf_counter()))
                elif data.vkCode == inject.VK_ESCAPE:
                    # With Ctrl as the hotkey, Esc while holding it is Ctrl+Esc, which opens Start.
                    swallow = self._hook_hotkey_held
                    if down:
                        self.post(Cancel())
                elif down:
                    self.post(OtherKeyDown())
        except Exception:
            log.exception("keyboard hook")
        if swallow:
            self._listener.suppress_event()
        # False keeps pynput from translating the key; the event still reaches the focused app.
        return False

    def _run(self) -> None:
        while True:
            awaiting = isinstance(self._state, AwaitingRelease)
            try:
                event = self._events.get(timeout=RELEASE_POLL_S if awaiting else None)
            except queue.Empty:
                if user32.GetAsyncKeyState(self._config.hotkey_vk) & 0x8000:
                    continue
                log.warning("hotkey release was not seen by the hook; using the key state")
                event = HotkeyUp(time.perf_counter())
            if event is None:
                return
            try:
                self._handle(event)
            except Exception:
                log.exception("handling %s", type(event).__name__)

    def _handle(self, event: Event) -> None:
        if isinstance(event, HotkeyUp):
            self._released_at = event.t
        previous = self._state
        self._state, effects = step(previous, event, self._config.tap_threshold_ms / 1000)
        if self._state != previous:
            log.info("%s -> %s", previous, self._state)
            self._on_state(self._state)
        for effect in effects:
            self._execute(effect)

    def _execute(self, effect: Effect) -> None:
        match effect:
            case StartRecording():
                try:
                    self._recorder.start()
                except Exception as e:
                    log.exception("microphone did not start")
                    self.post(RecordingFailed(str(e)))
                else:
                    log.info("recording from %r", self._recorder.device_name)
            case StopAndTranscribe():
                try:
                    audio = self._recorder.stop()
                except Exception as e:
                    log.exception("microphone did not stop cleanly")
                    self.post(RecordingFailed(str(e)))
                else:
                    self._worker.submit(self._transcribe, audio)
            case DiscardRecording():
                self._recorder.discard()
            case Paste(text):
                since_release_ms = (time.perf_counter() - self._released_at) * 1000
                try:
                    self._paste(text + " " if self._config.append_space else text)
                except Exception:
                    log.exception("paste failed")
                    self._cues.play(Cue.ERROR)
                else:
                    log.info("pasted %d chars, sent %.0f ms after the hotkey release", len(text), since_release_ms)
            case PlaySound(cue):
                self._cues.play(cue)

    def _transcribe(self, audio: Audio) -> None:
        t0 = time.perf_counter()
        try:
            text = self._transcriber.transcribe(audio)
        except Exception as e:
            log.exception("transcription failed")
            self.post(RecordingFailed(str(e)))
            return
        ms = (time.perf_counter() - t0) * 1000
        log.info("transcribed %.1f s at %d Hz in %.0f ms: %d chars", audio.seconds, audio.rate, ms, len(text))
        self.post(TranscriptReady(text, next(self._request_ids)))


class Cues:
    NOTES_HZ = {Cue.START: (600, 900), Cue.STOP: (900, 600), Cue.CANCEL: (450,), Cue.ERROR: (300, 300)}
    RATE = 44_100
    NOTE_S = 0.06
    GAP_S = 0.02
    VOLUME = 0.12

    def __init__(self, enabled: bool) -> None:
        self._paths: dict[Cue, Path] = {}
        if enabled:
            folder = Path(tempfile.gettempdir()) / "parakeet-dictate"
            folder.mkdir(exist_ok=True)
            for cue, notes in self.NOTES_HZ.items():
                path = folder / f"{cue.value}.wav"
                self._write(path, notes)
                self._paths[cue] = path

    def play(self, cue: Cue) -> None:
        path = self._paths.get(cue)
        if path:
            winsound.PlaySound(str(path), winsound.SND_FILENAME | winsound.SND_ASYNC | winsound.SND_NODEFAULT)

    def _write(self, path: Path, notes: tuple[int, ...]) -> None:
        n = int(self.NOTE_S * self.RATE)
        fade = int(0.008 * self.RATE)
        samples: list[int] = []
        for hz in notes:
            for i in range(n):
                envelope = min(1.0, i / fade, (n - i) / fade)
                samples.append(int(32767 * self.VOLUME * envelope * math.sin(2 * math.pi * hz * i / self.RATE)))
            samples.extend([0] * int(self.GAP_S * self.RATE))
        with wave.open(str(path), "wb") as f:
            f.setnchannels(1)
            f.setsampwidth(2)
            f.setframerate(self.RATE)
            f.writeframes(struct.pack(f"<{len(samples)}h", *samples))


class Tray:
    COLORS = {"idle": "#9e9e9e", "recording": "#e53935", "transcribing": "#ffb300"}

    def __init__(self, on_quit: Callable[[], None]) -> None:
        self.provider = ""
        self.icon = pystray.Icon(
            "parakeet-dictate",
            self._ring("#9e9e9e"),
            "Parakeet Dictate: loading model",
            menu=pystray.Menu(
                pystray.MenuItem("Open log", lambda: os.startfile(LOG_PATH)),
                pystray.MenuItem("Quit", lambda: on_quit()),
            ),
        )

    def show_state(self, state: State) -> None:
        match state:
            case Recording():
                status = "recording"
            case Transcribing() | AwaitingRelease():
                status = "transcribing"
            case _:
                status = "idle"
        self.icon.icon = self._disc(self.COLORS[status])
        self.icon.title = f"Parakeet Dictate ({self.provider}): {'ready' if status == 'idle' else status}"

    def show_error(self, message: str) -> None:
        self.icon.icon = self._ring("#e53935")
        self.icon.title = f"Parakeet Dictate failed: {message}"[:120]

    @staticmethod
    def _disc(color: str) -> Image.Image:
        image = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
        ImageDraw.Draw(image).ellipse((6, 6, 58, 58), fill=color)
        return image

    @staticmethod
    def _ring(color: str) -> Image.Image:
        image = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
        ImageDraw.Draw(image).ellipse((8, 8, 56, 56), outline=color, width=8)
        return image


def acquire_single_instance() -> int | None:
    ctypes.set_last_error(0)
    handle = kernel32.CreateMutexW(None, False, MUTEX_NAME)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    if ctypes.get_last_error() == ERROR_ALREADY_EXISTS:
        kernel32.CloseHandle(handle)
        return None
    return handle


def signal_quit() -> bool:
    handle = kernel32.OpenEventW(EVENT_MODIFY_STATE, False, QUIT_EVENT_NAME)
    if not handle:
        return False
    kernel32.SetEvent(handle)
    kernel32.CloseHandle(handle)
    return True


def setup_logging() -> None:
    # pythonw has no console; libraries that write to a None stream would crash.
    if sys.stdout is None:
        sys.stdout = open(os.devnull, "w")
    if sys.stderr is None:
        sys.stderr = open(os.devnull, "w")
    handler = RotatingFileHandler(LOG_PATH, maxBytes=1_000_000, backupCount=2, encoding="utf-8")
    handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(threadName)s %(name)s: %(message)s"))
    logging.basicConfig(level=logging.INFO, handlers=[handler])
    logging.captureWarnings(True)
    sys.excepthook = lambda *exc: log.critical("uncaught exception", exc_info=exc)
    threading.excepthook = lambda args: log.critical(
        "uncaught exception in %s", args.thread, exc_info=(args.exc_type, args.exc_value, args.exc_traceback)
    )


def main() -> int:
    parser = argparse.ArgumentParser(prog="parakeet-dictate", description="Push-to-talk dictation with Parakeet.")
    parser.add_argument("--quit", action="store_true", help="stop the running instance")
    args = parser.parse_args()
    setup_logging()

    if args.quit:
        stopped = signal_quit()
        log.info("--quit: %s", "signalled the running instance" if stopped else "no instance running")
        return 0 if stopped else 1

    mutex = acquire_single_instance()
    if mutex is None:
        log.info("another instance is already running; exiting")
        return 0
    log.info("starting, pid %d", os.getpid())
    try:
        config = load_config()
    except ConfigError as e:
        log.error("invalid config: %s", e)
        return 1

    quit_event = kernel32.CreateEventW(None, True, False, QUIT_EVENT_NAME)
    tray = Tray(on_quit=lambda: tray.icon.stop())
    cues = Cues(config.sounds)
    stopping = threading.Event()
    running: list[Dictation] = []

    def load() -> None:
        try:
            transcriber = load_transcriber(config.model_dir, config.provider)
            transcriber.warm_up(default_input_rate())
            if stopping.is_set():
                return
            tray.provider = transcriber.label
            dictation = Dictation(config, recorder=Recorder(), transcriber=transcriber, cues=cues, on_state=tray.show_state)
            dictation.start()
            running.append(dictation)
            tray.show_state(Idle())
            log.info("ready: provider %s, hotkey %s", transcriber.provider, config.hotkey)
        except Exception as e:
            log.exception("startup failed")
            tray.show_error(str(e) or type(e).__name__)
            cues.play(Cue.ERROR)

    def wait_for_quit() -> None:
        if kernel32.WaitForSingleObject(quit_event, INFINITE) == WAIT_OBJECT_0:
            log.info("quit requested by another process")
            tray.icon.stop()

    loader = threading.Thread(target=load, name="loader")
    loader.start()
    threading.Thread(target=wait_for_quit, name="quit-watcher", daemon=True).start()
    tray.icon.run()

    log.info("shutting down")
    stopping.set()
    loader.join()
    for dictation in running:
        dictation.stop()
    kernel32.CloseHandle(quit_event)
    kernel32.CloseHandle(mutex)
    log.info("stopped")
    return 0
