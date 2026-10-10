import argparse
import ctypes
import itertools
import json
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
from .config import LOG_PATH, OVERLAY_CHOICES, TRAY_CHOICES_PATH, Config, ConfigError, load_api_key, load_config
from .overlay import Overlay
from .polish import GlmClient, PolishError
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
    Polish,
    PolishFailed,
    Polished,
    Polishing,
    Recording,
    RecordingFailed,
    StartRecording,
    State,
    StopAndTranscribe,
    Transcribing,
    TranscriptReady,
    Transition,
    step,
)
from .styles import STYLES
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
        on_transition: Callable[[Transition], None] = lambda transition: None,
        glm: GlmClient | None = None,
        polish_enabled: Callable[[], bool] = lambda: True,
    ) -> None:
        self._config = config
        self._recorder = recorder
        self._transcriber = transcriber
        self._paste = paste
        self._cues = cues
        self._on_transition = on_transition
        self._glm = glm
        self._polish_enabled = polish_enabled
        self._events: queue.Queue[Event | None] = queue.Queue()
        self._state: State = Idle()
        self._released_at = 0.0
        self._worker = ThreadPoolExecutor(max_workers=1, thread_name_prefix="transcribe")
        # Two workers, so a request still running after Esc never delays the next dictation's.
        self._polisher = ThreadPoolExecutor(max_workers=2, thread_name_prefix="polish")
        self._request_ids = itertools.count(1)
        self._controller = threading.Thread(target=self._run, name="controller", daemon=True)
        self._listener = keyboard.Listener(win32_event_filter=self._on_key)
        self._hook_hotkey_held = False
        self._hook_esc_swallowed = False

    def start(self) -> None:
        self._controller.start()
        self._listener.start()
        self._listener.wait()

    def stop(self) -> None:
        self._listener.stop()
        self._events.put(None)
        self._controller.join(timeout=10)
        self._worker.shutdown(wait=True, cancel_futures=True)
        self._polisher.shutdown(wait=False, cancel_futures=True)
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
                    if down:
                        # With Ctrl as the hotkey, Esc while holding it is Ctrl+Esc, which opens Start.
                        # While polishing, Esc pastes the raw transcript into the focused app, which
                        # must not also act on the Esc. The controller only ever replaces _state with
                        # an immutable state, so reading it from this thread is safe.
                        self._hook_esc_swallowed = self._hook_hotkey_held or isinstance(self._state, Polishing)
                        self.post(Cancel())
                    swallow = self._hook_esc_swallowed
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
        self._state, effects = step(previous, event, self._config.tap_threshold_ms / 1000, self._polish_on())
        if self._state != previous:
            log.info("%s -> %s on %s", previous, self._state, event)
            if isinstance(event, PolishFailed):
                log.warning("post-processing failed (%s); pasting the raw transcript", event.reason)
            if isinstance(event, OtherKeyDown) and isinstance(previous, Recording):
                # Data for the overlay's show delay: how soon a real shortcut's second key arrives.
                log.info("shortcut: another key %.0f ms after the hotkey", (time.perf_counter() - previous.pressed_at) * 1000)
            # Before the effects run: a paste blocks this thread for the clipboard restore, and the
            # observers should see the transition when it happens.
            self._on_transition(Transition(previous, self._state, event, tuple(effects)))
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
                    if self._polish_on():
                        self._polisher.submit(self._glm.preconnect)
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
            case Polish(text, request_id):
                self._polisher.submit(self._polish, text, request_id)
                # httpx times each network step separately and never times the DNS lookup, so this
                # timer bounds the whole wait. When the request finishes first, the reducer ignores
                # the timer's PolishFailed because its id is no longer current.
                timeout_s = self._config.postprocess.timeout_s
                deadline = PolishFailed(request_id, f"no reply within {timeout_s:g} s")
                timer = threading.Timer(timeout_s, self.post, [deadline])
                timer.daemon = True
                timer.start()

    def _polish_on(self) -> bool:
        return self._glm is not None and self._polish_enabled()

    def _polish(self, text: str, request_id: int) -> None:
        try:
            result = Polished(request_id, self._glm.polish(text))
        except PolishError as e:
            result = PolishFailed(request_id, str(e))
        except Exception as e:
            log.exception("post-processing crashed")
            result = PolishFailed(request_id, type(e).__name__)
        self.post(result)

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


class TrayChoices:
    """What the tray menu sets, saved in one file so it survives a restart. A choice made in the tray
    wins over its config.toml setting; a key never chosen, or saved invalid, falls back to the config."""

    VALID: dict[str, Callable[[object], bool]] = {
        "postprocess": lambda value: type(value) is bool,
        "overlay": lambda value: isinstance(value, str) and value in OVERLAY_CHOICES,
    }

    def __init__(self, config: Config, path: Path) -> None:
        self._path = path
        self._config = {"postprocess": config.postprocess.enabled, "overlay": config.overlay}
        # Only keys chosen in the tray, so a choice of one never pins the other's config value.
        self._chosen: dict[str, object] = {}
        try:
            saved = json.loads(path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            return
        except (OSError, ValueError):
            saved = None
        if not isinstance(saved, dict):
            log.warning("ignoring %s: expected a JSON object", path.name)
            return
        for key, value in saved.items():
            if key in self.VALID and self.VALID[key](value):
                self._chosen[key] = value
            else:
                log.warning("ignoring %r in %s: %r", key, path.name, value)

    @property
    def postprocess(self) -> bool:
        return self._chosen.get("postprocess", self._config["postprocess"])

    @property
    def overlay(self) -> str:
        """A style name or "off"."""
        return self._chosen.get("overlay", self._config["overlay"])

    def choose(self, key: str, value: object) -> None:
        self._chosen[key] = value
        temporary = self._path.with_name(self._path.name + ".tmp")
        try:
            temporary.write_text(json.dumps(self._chosen), encoding="utf-8")
            os.replace(temporary, self._path)
        except OSError:
            log.exception("could not save the tray choices")


class Tray:
    COLORS = {"idle": "#9e9e9e", "recording": "#e53935", "transcribing": "#ffb300", "polishing": "#8e24aa"}

    def __init__(self, on_quit: Callable[[], None], choices: TrayChoices, glm_available: bool) -> None:
        self.provider = ""
        self._choices = choices
        self.icon = pystray.Icon(
            "parakeet-dictate",
            self._ring("#9e9e9e"),
            "Parakeet Dictate: loading model",
            menu=pystray.Menu(
                pystray.MenuItem(
                    "Post-process with GLM" if glm_available else "Post-process with GLM (no ZAI_API_KEY)",
                    lambda: self._toggle_postprocess(),
                    checked=lambda item: glm_available and choices.postprocess,
                    enabled=glm_available,
                ),
                pystray.Menu.SEPARATOR,
                pystray.MenuItem("Open log", lambda: os.startfile(LOG_PATH)),
                pystray.MenuItem("Quit", lambda: on_quit()),
            ),
        )

    def _toggle_postprocess(self) -> None:
        on = not self._choices.postprocess
        log.info("post-processing turned %s from the tray", "on" if on else "off")
        self._choices.choose("postprocess", on)
        self.icon.update_menu()

    def show_state(self, state: State) -> None:
        match state:
            case Recording():
                status = "recording"
            case Transcribing() | AwaitingRelease():
                status = "transcribing"
            case Polishing():
                status = "polishing"
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


def open_overlay(recorder: Recorder, style: str) -> Overlay | None:
    try:
        return Overlay(STYLES[style], lambda: recorder.meter.snapshot())
    except OSError:
        log.exception("could not create the overlay window; dictating without it")
        return None


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
    logging.getLogger("httpx2").setLevel(logging.WARNING)
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
    api_key = load_api_key()
    glm = GlmClient(config.postprocess, api_key) if api_key else None
    choices = TrayChoices(config, TRAY_CHOICES_PATH)
    tray = Tray(on_quit=lambda: tray.icon.stop(), choices=choices, glm_available=glm is not None)
    cues = Cues(config.sounds)
    stopping = threading.Event()
    running: list[Dictation] = []
    overlays: list[Overlay] = []

    def load() -> None:
        try:
            transcriber = load_transcriber(config.model_dir, config.provider)
            transcriber.warm_up(default_input_rate())
            if stopping.is_set():
                return
            tray.provider = transcriber.label
            recorder = Recorder()
            overlay = None if config.overlay == "off" else open_overlay(recorder, config.overlay)
            if overlay is not None:
                overlays.append(overlay)

            def on_transition(transition: Transition) -> None:
                tray.show_state(transition.state)
                if overlay is not None:
                    overlay.notify(transition)

            dictation = Dictation(
                config,
                recorder=recorder,
                transcriber=transcriber,
                cues=cues,
                on_transition=on_transition,
                glm=glm,
                polish_enabled=lambda: choices.postprocess,
            )
            dictation.start()
            running.append(dictation)
            tray.show_state(Idle())
            if glm is None:
                postprocessing = "unavailable, no ZAI_API_KEY in the environment or .env"
            else:
                settings = f"{config.postprocess.model}, reasoning effort {config.postprocess.reasoning_effort}"
                postprocessing = f"{'on' if choices.postprocess else 'off'} ({settings})"
            log.info(
                "ready: provider %s, hotkey %s, post-processing %s, overlay %s",
                transcriber.provider, config.hotkey, postprocessing, config.overlay if overlay is not None else "off",
            )
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
    for overlay in overlays:
        overlay.close()
    if glm is not None:
        glm.close()
    kernel32.CloseHandle(quit_event)
    kernel32.CloseHandle(mutex)
    log.info("stopped")
    return 0
