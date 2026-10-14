import argparse
import ctypes
import itertools
import json
import logging
import math
import os
import queue
import struct
import subprocess
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
from .app_rules import combined_instructions
from .config import (
    LOG_PATH,
    OVERLAY_CHOICES,
    ROOT,
    SERVER_TOKEN_PATH,
    TRAY_CHOICES_PATH,
    Config,
    ConfigError,
    PostprocessConfig,
    load_api_key,
    load_config,
)
from .foreground import foreground_executable
from .overlay import Overlay
from .overlay_frame import Renderer
from .polish import Polisher, PolishError, create_polisher
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
from .server import DictationServer
from .styles import STYLES
from .transcriber import SharedModel, load_transcriber

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
LAUNCH_EVENT_NAME = "Local\\parakeet-dictate-launched"
RELOAD_EVENT_NAME = "Local\\parakeet-dictate-reload"
ERROR_ALREADY_EXISTS = 183
EVENT_MODIFY_STATE = 0x0002
INFINITE = 0xFFFFFFFF
WAIT_OBJECT_0 = 0
WM_KEYDOWN, WM_SYSKEYDOWN = 0x0100, 0x0104
RELEASE_POLL_S = 0.05


class FinishDictation:
    """Stop accepting recordings once the current dictation and hotkey release finish."""


class Dictation:
    def __init__(
        self,
        config: Config,
        *,
        recorder: Recorder,
        transcriber: SharedModel,
        paste: Callable[[str], None] = inject.paste,
        cues: "Cues",
        on_transition: Callable[[Transition], None] = lambda transition: None,
        polisher: Polisher | None = None,
        polish_enabled: Callable[[], bool] = lambda: True,
        foreground_app: Callable[[], str | None] = foreground_executable,
    ) -> None:
        self._config = config
        self._recorder = recorder
        self._transcriber = transcriber
        self._paste = paste
        self._cues = cues
        self._on_transition = on_transition
        self._polish_client = polisher
        self._polish_enabled = polish_enabled
        self._foreground_app = foreground_app
        self._instructions = ""
        self._events: queue.Queue[Event | FinishDictation | None] = queue.Queue()
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

    def finish(self) -> None:
        self._events.put(FinishDictation())
        self._controller.join()

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
        finishing = False
        while True:
            if finishing and self._state == Idle():
                return
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
            if isinstance(event, FinishDictation):
                finishing = True
                continue
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
                self._instructions = self._read_rules()
                try:
                    self._recorder.start()
                except Exception as e:
                    log.exception("microphone did not start")
                    self.post(RecordingFailed(str(e)))
                else:
                    log.info("recording from %r", self._recorder.device_name)
                    if self._polish_on():
                        self._polisher.submit(self._polish_client.preconnect)
            case StopAndTranscribe():
                self._instructions = self._read_rules()
                if self._instructions and self._polish_client is None:
                    log.warning("app rules need %s; pasting the raw transcript", self._config.postprocess.api_key_name)
                try:
                    audio = self._recorder.stop()
                except Exception as e:
                    log.exception("microphone did not stop cleanly")
                    self.post(RecordingFailed(str(e)))
                else:
                    self._worker.submit(self._transcribe, audio)
            case DiscardRecording():
                self._instructions = ""
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
                # Capture now: Esc can start another dictation before a queued request runs.
                self._polisher.submit(self._polish, text, request_id, self._instructions)
                # httpx times each network step separately and never times the DNS lookup, so this
                # timer bounds the whole wait. When the request finishes first, the reducer ignores
                # the timer's PolishFailed because its id is no longer current.
                timeout_s = self._config.postprocess.timeout_s
                deadline = PolishFailed(request_id, f"no reply within {timeout_s:g} s")
                timer = threading.Timer(timeout_s, self.post, [deadline])
                timer.daemon = True
                timer.start()

    def _polish_on(self) -> bool:
        return self._polish_client is not None and (self._polish_enabled() or bool(self._instructions))

    def _read_rules(self) -> str:
        if not self._config.app_rules:
            return ""
        return combined_instructions(self._config.app_rules, self._foreground_app())

    def _polish(self, text: str, request_id: int, instructions: str) -> None:
        try:
            result = Polished(request_id, self._polish_client.polish(text, instructions))
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

    _VALID: dict[str, Callable[[object], bool]] = {
        "postprocess": lambda value: type(value) is bool,
        "overlay": lambda value: isinstance(value, str) and value in OVERLAY_CHOICES,
        "server": lambda value: type(value) is bool,
    }

    def __init__(self, config: Config, path: Path) -> None:
        self._path = path
        self._config = {"postprocess": config.postprocess.enabled, "overlay": config.overlay, "server": config.server.enabled}
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
            if key in self._VALID and self._VALID[key](value):
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

    @property
    def server(self) -> bool:
        return self._chosen.get("server", self._config["server"])

    def choose(self, key: str, value: object) -> None:
        self._chosen[key] = value
        temporary = self._path.with_name(self._path.name + ".tmp")
        try:
            temporary.write_text(json.dumps(self._chosen), encoding="utf-8")
            os.replace(temporary, self._path)
        except OSError:
            log.exception("could not save the tray choices")


class ActiveOverlay:
    """Which overlay style shows, if any, and the one window that draws it. The window opens the first
    time a style is active, so an app started with "off" creates none until the tray picks a style."""

    def __init__(self, style: str, open_window: Callable[[Renderer], Overlay], cues: Cues) -> None:
        self.style = "off"
        self._open_window = open_window
        self._cues = cues
        self._overlay: Overlay | None = None
        self._last: Transition | None = None
        # The controller notifies while the tray switches; without it a resync of an older transition
        # could reach the overlay after a newer one.
        self._lock = threading.Lock()
        self.switch(style)

    def switch(self, style: str) -> bool:
        """Draw style from now on, or "off". False when the window could not be created; it stays off."""
        renderer = STYLES.get(style)
        with self._lock:
            if renderer is not None and self._overlay is None:
                try:
                    self._overlay = self._open_window(renderer)
                except OSError:
                    log.exception("could not create the overlay window; dictating without it")
                    self._cues.play(Cue.ERROR)
                    return False
            elif self._overlay is not None:
                self._overlay.restyle(renderer)
            self.style = style
            if renderer is not None and self._last is not None:
                # Off dropped the dictation on screen; without this it would only reappear on the next transition.
                self._overlay.notify(self._last)
            return True

    def notify(self, transition: Transition) -> None:
        with self._lock:
            self._last = transition
            if self.style != "off":
                self._overlay.notify(transition)

    def close(self) -> None:
        if self._overlay is not None:
            self._overlay.close()


class Tray:
    COLORS = {"idle": "#9e9e9e", "recording": "#e53935", "transcribing": "#ffb300", "polishing": "#8e24aa"}

    def __init__(
        self,
        on_quit: Callable[[], None],
        choices: TrayChoices,
        overlay: ActiveOverlay,
        polish_available: bool,
        server: DictationServer,
        cues: Cues,
        postprocess: PostprocessConfig = PostprocessConfig(),
        on_settings: Callable[[], None] = lambda: None,
    ) -> None:
        self.provider = ""
        self._choices = choices
        self._overlay = overlay
        self._server = server
        self._cues = cues
        self.icon = pystray.Icon(
            "parakeet-dictate",
            self._ring("#9e9e9e"),
            "Parakeet Dictate: loading model",
            menu=pystray.Menu(
                pystray.MenuItem(
                    f"Post-process with {postprocess.label}" + ("" if polish_available else f" (no {postprocess.api_key_name})"),
                    lambda: self._toggle_postprocess(),
                    checked=lambda item: polish_available and choices.postprocess,
                    enabled=polish_available,
                ),
                pystray.MenuItem("Overlay", pystray.Menu(*map(self._overlay_item, OVERLAY_CHOICES))),
                pystray.MenuItem("Server for Mac", lambda: self._toggle_server(), checked=lambda item: server.running),
                pystray.MenuItem(
                    "Copy Mac connection info",
                    lambda: threading.Thread(target=self._copy_connection_info, name="connection-info", daemon=True).start(),
                    enabled=lambda item: server.running,
                ),
                pystray.Menu.SEPARATOR,
                pystray.MenuItem("Settings…", lambda: on_settings(), default=True),
                pystray.MenuItem("Open log", lambda: os.startfile(LOG_PATH)),
                pystray.MenuItem("Quit", lambda: on_quit()),
            ),
        )

    def _overlay_item(self, style: str) -> pystray.MenuItem:
        # A function per item binds each style; pystray passes the icon to a one-argument action,
        # so a style=style default argument would receive the icon instead.
        return pystray.MenuItem(
            style.capitalize(),
            lambda: self._switch_overlay(style),
            checked=lambda item: self._overlay.style == style,
            radio=True,
        )

    def _switch_overlay(self, style: str) -> None:
        if self._overlay.switch(style):
            log.info("overlay switched to %s from the tray", style)
            self._choices.choose("overlay", style)
        self.icon.update_menu()

    def _toggle_postprocess(self) -> None:
        on = not self._choices.postprocess
        log.info("post-processing turned %s from the tray", "on" if on else "off")
        self._choices.choose("postprocess", on)
        self.icon.update_menu()

    def start_server(self) -> bool:
        try:
            self._server.start()
        except OSError:
            log.exception("could not start the server for Mac")
            self._cues.play(Cue.ERROR)
            return False
        return True

    def _toggle_server(self) -> None:
        if self._server.running:
            self._server.stop()
            self._choices.choose("server", False)
            log.info("server for Mac turned off from the tray")
        elif self.start_server():
            self._choices.choose("server", True)
            log.info("server for Mac turned on from the tray")
        self.icon.update_menu()

    def _copy_connection_info(self) -> None:
        # Its own thread: finding the LAN address takes PowerShell a second or two.
        info = self._server.connection_info()
        if info is None:
            self._cues.play(Cue.ERROR)
            self.notify("Could not find this PC's address on the local network.")
            return
        try:
            inject.set_text(info, keep_out_of_history=True)
        except OSError:
            log.exception("could not copy the Mac connection info")
            self._cues.play(Cue.ERROR)
            return
        log.info("copied the Mac connection info: %s and the token", info.splitlines()[0])
        self.notify(f"Copied {info.splitlines()[0]} and the token. Paste them into the Mac app.")

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

    def notify(self, message: str) -> None:
        # A notification that fails, say before the tray window exists, must not stop the launch watcher.
        try:
            self.icon.notify(message, "Parakeet Dictate")
        except Exception:
            log.exception("could not show the notification %r", message)

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


def signal_event(name: str) -> bool:
    handle = kernel32.OpenEventW(EVENT_MODIFY_STATE, False, name)
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


def launch_background(*arguments: str) -> None:
    subprocess.Popen(
        [str(ROOT / ".venv" / "Scripts" / "pythonw.exe"), "-m", "parakeet_dictate", *arguments],
        cwd=ROOT, creationflags=subprocess.CREATE_NO_WINDOW,
    )


def main() -> int:
    parser = argparse.ArgumentParser(prog="parakeet-dictate", description="Push-to-talk dictation with Parakeet.")
    parser.add_argument("--quit", action="store_true", help="stop the running instance")
    parser.add_argument("--notify-ready", action="store_true", help="show a notification once the model is loaded")
    parser.add_argument("--settings", action="store_true", help="open the settings window")
    args = parser.parse_args()
    if args.settings:
        from .settings_window import main as settings_main
        return settings_main()
    setup_logging()

    if args.quit:
        stopped = signal_event(QUIT_EVENT_NAME)
        log.info("--quit: %s", "signalled the running instance" if stopped else "no instance running")
        return 0 if stopped else 1

    mutex = acquire_single_instance()
    if mutex is None:
        told = signal_event(LAUNCH_EVENT_NAME)
        log.info("another instance is already running; %s", "asked it to say so" if told else "it is still starting")
        return 0
    log.info("starting, pid %d", os.getpid())
    # Publish before reading settings, so a save during startup can request another reload.
    reload_event = kernel32.CreateEventW(None, False, False, RELOAD_EVENT_NAME)
    try:
        config = load_config()
    except ConfigError as e:
        log.error("invalid config: %s", e)
        kernel32.CloseHandle(reload_event)
        kernel32.CloseHandle(mutex)
        return 1

    quit_event = kernel32.CreateEventW(None, True, False, QUIT_EVENT_NAME)
    # Auto-reset, so each later launch wakes the watcher once.
    launch_event = kernel32.CreateEventW(None, False, False, LAUNCH_EVENT_NAME)
    hint = f"Hold {config.hotkey_label} to dictate."
    api_key = load_api_key(name=config.postprocess.api_key_name)
    polisher = create_polisher(config.postprocess, api_key) if api_key else None
    choices = TrayChoices(config, TRAY_CHOICES_PATH)
    cues = Cues(config.sounds)
    recorder = Recorder()
    overlay = ActiveOverlay(choices.overlay, lambda renderer: Overlay(renderer, lambda: recorder.meter.snapshot()), cues)
    model = SharedModel()
    server = DictationServer(
        config.server,
        model=model,
        polisher=polisher,
        polish_enabled=lambda: choices.postprocess,
        timeout_s=config.postprocess.timeout_s,
        token_path=SERVER_TOKEN_PATH,
    )
    tray = Tray(
        on_quit=lambda: tray.icon.stop(),
        choices=choices,
        overlay=overlay,
        polish_available=polisher is not None,
        server=server,
        cues=cues,
        postprocess=config.postprocess,
        on_settings=lambda: launch_background("--settings"),
    )
    # Before the model loads, so a Mac sees 503 "still loading" instead of no server.
    if choices.server:
        tray.start_server()
    stopping = threading.Event()
    restarting = threading.Event()
    running: list[Dictation] = []

    def load() -> None:
        try:
            transcriber = load_transcriber(config.model_dir, config.provider)
            transcriber.warm_up(default_input_rate())
            if stopping.is_set():
                return
            model.load(transcriber)
            tray.provider = transcriber.label

            def on_transition(transition: Transition) -> None:
                tray.show_state(transition.state)
                overlay.notify(transition)

            dictation = Dictation(
                config,
                recorder=recorder,
                transcriber=model,
                cues=cues,
                on_transition=on_transition,
                polisher=polisher,
                polish_enabled=lambda: choices.postprocess,
            )
            dictation.start()
            running.append(dictation)
            tray.show_state(Idle())
            if polisher is None:
                postprocessing = f"unavailable, no {config.postprocess.api_key_name} in the environment or .env"
            else:
                effort = config.postprocess.effort if config.postprocess.provider == "anthropic" else config.postprocess.reasoning_effort
                settings = f"{config.postprocess.model}, effort {effort}"
                if config.postprocess.provider == "anthropic":
                    settings += f", thinking {'on' if config.postprocess.thinking else 'off'}"
                postprocessing = f"{'on' if choices.postprocess else 'off'} ({settings})"
            log.info(
                "ready: provider %s, hotkey %s, post-processing %s, server for Mac %s, overlay %s",
                transcriber.provider, config.hotkey, postprocessing,
                f"on port {server.port}" if server.running else "off", overlay.style,
            )
            if args.notify_ready:
                tray.notify(f"Ready. {hint}")
        except Exception as e:
            log.exception("startup failed")
            tray.show_error(str(e) or type(e).__name__)
            cues.play(Cue.ERROR)

    def wait_for_quit() -> None:
        if kernel32.WaitForSingleObject(quit_event, INFINITE) == WAIT_OBJECT_0:
            log.info("quit requested by another process")
            tray.icon.stop()

    def wait_for_launches() -> None:
        while kernel32.WaitForSingleObject(launch_event, INFINITE) == WAIT_OBJECT_0:
            log.info("launched again while running")
            tray.notify(f"Parakeet Dictate is already running. {hint}")

    loader = threading.Thread(target=load, name="loader")

    def wait_for_settings() -> None:
        if kernel32.WaitForSingleObject(reload_event, INFINITE) != WAIT_OBJECT_0:
            return
        log.info("settings saved; finishing the current dictation before reloading")
        loader.join()
        for dictation in running:
            dictation.finish()
        if not stopping.is_set():
            restarting.set()
            tray.icon.stop()

    loader.start()
    threading.Thread(target=wait_for_quit, name="quit-watcher", daemon=True).start()
    threading.Thread(target=wait_for_launches, name="launch-watcher", daemon=True).start()
    threading.Thread(target=wait_for_settings, name="settings-watcher", daemon=True).start()
    tray.icon.run()

    log.info("shutting down")
    stopping.set()
    loader.join()
    server.stop()
    for dictation in running:
        dictation.stop()
    overlay.close()
    if polisher is not None:
        polisher.close()
    kernel32.CloseHandle(quit_event)
    kernel32.CloseHandle(launch_event)
    kernel32.CloseHandle(reload_event)
    kernel32.CloseHandle(mutex)
    log.info("stopped")
    if restarting.is_set():
        launch_background("--notify-ready")
    return 0
