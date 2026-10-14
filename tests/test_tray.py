import io
import json
import socket
import sys
import threading
import time
import uuid
import wave
from collections.abc import Callable
from dataclasses import replace
from types import SimpleNamespace

import httpx2
import pystray
import pytest

from parakeet_dictate import app
from parakeet_dictate.app import ActiveOverlay, Tray, TrayChoices
from parakeet_dictate.config import Config, PostprocessConfig, ServerConfig
from parakeet_dictate.polish import ClaudeClient, create_polisher
from parakeet_dictate.reducer import Cue, HotkeyDown, Idle, Mode, Recording, Transition
from parakeet_dictate.server import DictationServer
from parakeet_dictate.styles import STYLES
from parakeet_dictate.transcriber import SharedModel

RECORDING = Transition(Idle(), Recording(Mode.HOLD, 0.0), HotkeyDown(0.0), ())


def _silence() -> bytes:
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(16_000)
        f.writeframes(bytes(32_000))
    return buffer.getvalue()


SECOND_OF_SILENCE = _silence()


class FakeOverlay:
    def __init__(self, renderer) -> None:
        self.renderer = renderer
        self.notified: list[Transition] = []
        self.closed = False

    def restyle(self, renderer) -> None:
        self.renderer = renderer

    def notify(self, transition: Transition) -> None:
        self.notified.append(transition)

    def close(self) -> None:
        self.closed = True


class FakeCues:
    def __init__(self) -> None:
        self.played: list[Cue] = []

    def play(self, cue: Cue) -> None:
        self.played.append(cue)


class Windows:
    """An overlay factory that records each window it opens."""

    def __init__(self, fail: bool = False) -> None:
        self.opened: list[FakeOverlay] = []
        self.fail = fail

    def __call__(self, renderer) -> FakeOverlay:
        if self.fail:
            raise OSError("CreateWindowExW failed")
        self.opened.append(FakeOverlay(renderer))
        return self.opened[-1]


def test_starting_off_creates_no_window_and_forwards_nothing():
    windows = Windows()
    overlay = ActiveOverlay("off", windows, FakeCues())
    overlay.notify(RECORDING)
    assert (overlay.style, windows.opened) == ("off", [])


def test_switching_opens_one_window_and_restyles_it_from_then_on():
    windows = Windows()
    overlay = ActiveOverlay("off", windows, FakeCues())
    assert overlay.switch("bars")
    assert [w.renderer for w in windows.opened] == [STYLES["bars"]]
    window = windows.opened[0]

    assert overlay.switch("fluid")
    assert (overlay.style, len(windows.opened), window.renderer) == ("fluid", 1, STYLES["fluid"])
    overlay.notify(RECORDING)
    assert window.notified == [RECORDING]

    assert overlay.switch("off")
    assert window.renderer is None
    overlay.notify(RECORDING)
    assert window.notified == [RECORDING], "off still forwards transitions"

    assert overlay.switch("bars")
    assert (len(windows.opened), window.renderer) == (1, STYLES["bars"])
    assert window.notified == [RECORDING, RECORDING], "turning on mid-dictation did not resync the current state"
    overlay.close()
    assert window.closed


def test_a_window_that_fails_to_open_leaves_the_overlay_off_with_the_error_cue():
    cues = FakeCues()
    overlay = ActiveOverlay("fluid", Windows(fail=True), cues)
    assert overlay.style == "off"
    assert not overlay.switch("bars")
    assert (overlay.style, cues.played) == ("off", [Cue.ERROR, Cue.ERROR])
    overlay.notify(RECORDING)
    overlay.close()


def overlay_items(icon: pystray.Icon) -> list[pystray.MenuItem]:
    (submenu,) = [item.submenu for item in icon.menu.items if item.text == "Overlay"]
    return list(submenu.items)


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def local_server(tmp_path, port: int = 0) -> DictationServer:
    return DictationServer(
        ServerConfig(bind="127.0.0.1", port=port), model=SharedModel(), polisher=None, polish_enabled=lambda: False,
        timeout_s=1.0, token_path=tmp_path / "server_token.txt",
    )


def make_tray(tmp_path, overlay: ActiveOverlay, server: DictationServer | None = None, cues: FakeCues | None = None) -> Tray:
    choices = TrayChoices(Config(), tmp_path / "tray_choices.json")
    return Tray(
        lambda: None, choices, overlay, polish_available=False, server=server or local_server(tmp_path), cues=cues or FakeCues()
    )


def test_the_overlay_menu_has_a_radio_item_per_style_and_off_that_switches_and_saves(tmp_path):
    path = tmp_path / "tray_choices.json"
    windows = Windows()
    overlay = ActiveOverlay("off", windows, FakeCues())
    tray = make_tray(tmp_path, overlay)
    items = overlay_items(tray.icon)
    assert [item.text for item in items] == ["Fluid", "Bars", "Off"]
    assert all(item.radio for item in items)
    assert [item.checked for item in items] == [False, False, True]

    for item, style in zip([items[1], items[0], items[2], items[1]], ["bars", "fluid", "off", "bars"]):
        item(tray.icon)
        assert overlay.style == style
        assert [i.checked for i in items] == [i.text.lower() == style for i in items]
        assert json.loads(path.read_text(encoding="utf-8")) == {"overlay": style}
    assert len(windows.opened) == 1
    assert [item.text for item in tray.icon.menu.items if item.text] == [
        "Post-process with GLM (no ZAI_API_KEY)", "Overlay", "Server for Mac", "Copy Mac connection info", "- - - -",
        "Settings…", "Open log", "Quit",
    ]


def server_items(icon: pystray.Icon) -> tuple[pystray.MenuItem, pystray.MenuItem]:
    items = {item.text: item for item in icon.menu.items}
    return items["Server for Mac"], items["Copy Mac connection info"]


def health_status(port: int, token: str) -> int | None:
    """None when nothing listens on the port."""
    try:
        return httpx2.get(
            f"http://127.0.0.1:{port}/v1/health", headers={"Authorization": f"Bearer {token}"}, timeout=5
        ).status_code
    except httpx2.ConnectError:
        return None


def test_the_server_item_starts_and_stops_the_server_and_saves_the_choice(tmp_path):
    port = free_port()
    server = local_server(tmp_path, port)
    tray = make_tray(tmp_path, ActiveOverlay("off", Windows(), FakeCues()), server)
    toggle, copy = server_items(tray.icon)
    saved = lambda: json.loads((tmp_path / "tray_choices.json").read_text(encoding="utf-8"))
    assert (toggle.checked, copy.enabled) == (False, False)
    try:
        for _ in range(2):
            toggle(tray.icon)
            assert (toggle.checked, copy.enabled, saved()) == (True, True, {"server": True})
            assert health_status(port, server.token) == 200
            toggle(tray.icon)
            assert (toggle.checked, copy.enabled, saved()) == (False, False, {"server": False})
            assert health_status(port, server.token) is None
    finally:
        server.stop()


def test_a_port_in_use_leaves_the_server_off_with_the_error_cue(tmp_path):
    cues = FakeCues()
    with socket.socket() as blocker:
        # As another Python HTTPServer would hold it: on Windows, two sockets that both set SO_REUSEADDR share a port.
        blocker.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        blocker.bind(("127.0.0.1", 0))
        blocker.listen()
        server = local_server(tmp_path, blocker.getsockname()[1])
        tray = make_tray(tmp_path, ActiveOverlay("off", Windows(), FakeCues()), server, cues)
        toggle, copy = server_items(tray.icon)
        toggle(tray.icon)
    assert (server.running, toggle.checked, copy.enabled, cues.played) == (False, False, False, [Cue.ERROR])
    assert not (tmp_path / "tray_choices.json").exists()


def wait_until(condition: Callable[[], object], what: str) -> None:
    deadline = time.perf_counter() + 5
    while not condition():
        assert time.perf_counter() < deadline, what
        time.sleep(0.01)


@pytest.fixture
def fake_main(tmp_path, monkeypatch, caplog) -> SimpleNamespace:
    """Runs main() with a fake tray loop, transcriber and Dictation, under mutex and event names of its
    own: no app mutex, no keyboard hook, nothing reaches a running app. `run` takes the command-line
    arguments and what to do inside the tray loop once the model is ready."""
    tag = uuid.uuid4().hex
    monkeypatch.setattr(app, "setup_logging", lambda: None)
    for name in ("MUTEX_NAME", "QUIT_EVENT_NAME", "LAUNCH_EVENT_NAME", "RELOAD_EVENT_NAME"):
        monkeypatch.setattr(app, name, f"{getattr(app, name)}-test-{tag}")
    monkeypatch.setattr(app, "TRAY_CHOICES_PATH", tmp_path / "tray_choices.json")
    monkeypatch.setattr(app, "SERVER_TOKEN_PATH", tmp_path / "server_token.txt")
    monkeypatch.setattr(app, "load_api_key", lambda **kwargs: None)
    monkeypatch.setattr(app, "default_input_rate", lambda: 16_000)
    harness = SimpleNamespace(
        config=Config(overlay="off", sounds=False), windows=Windows(), dictations=[], notifications=[],
        finished=threading.Event(), stopped=threading.Event(), launches=[],
    )
    # Whatever a test configures, the server listens on loopback only, on a port of the system's choosing.
    monkeypatch.setattr(
        app, "load_config", lambda: replace(harness.config, server=replace(harness.config.server, bind="127.0.0.1", port=0))
    )
    monkeypatch.setattr(app, "Overlay", lambda renderer, levels: harness.windows(renderer))
    monkeypatch.setattr(
        pystray.Icon, "notify", lambda icon, message, title=None: harness.notifications.append((message, title))
    )

    class Transcriber:
        label = provider = "fake"

        def warm_up(self, rate: int) -> None:
            pass

        def transcribe(self, audio) -> str:
            return "from the loaded model"

    monkeypatch.setattr(app, "load_transcriber", lambda model_dir, provider: Transcriber())

    class FakeDictation:
        def __init__(self, config, **parts) -> None:
            harness.dictations.append(parts)

        def start(self) -> None:
            pass

        def stop(self) -> None:
            pass

        def finish(self) -> None:
            harness.finished.set()

    monkeypatch.setattr(app, "Dictation", FakeDictation)
    monkeypatch.setattr(app, "launch_background", lambda *args: harness.launches.append(args))
    monkeypatch.setattr(pystray.Icon, "stop", lambda icon: harness.stopped.set())

    def run(*args: str, during: Callable[[pystray.Icon], None] = lambda icon: None) -> int:
        def loop(icon: pystray.Icon) -> None:
            ready = lambda: any(r.getMessage().startswith("ready:") for r in caplog.records)
            wait_until(ready, "the loader never logged ready")
            during(icon)

        monkeypatch.setattr(sys, "argv", ["parakeet-dictate", *args])
        monkeypatch.setattr(pystray.Icon, "run", loop)
        with caplog.at_level("INFO", logger="parakeet_dictate"):
            return app.main()

    harness.run = run
    return harness


def test_main_wires_the_tray_menu_to_the_overlay_and_the_dictation(fake_main, tmp_path, caplog):
    seen: dict[str, object] = {}

    def during(icon: pystray.Icon) -> None:
        on_transition = fake_main.dictations[0]["on_transition"]
        on_transition(RECORDING)
        seen["opened while off"] = len(fake_main.windows.opened)
        items = overlay_items(icon)
        seen["labels"] = [item.text for item in items]
        next(item for item in items if item.text == "Bars")(icon)
        seen["checked"] = [item.checked for item in items]
        on_transition(RECORDING)

    assert fake_main.run(during=during) == 0
    messages = [r.getMessage() for r in caplog.records]

    assert seen == {"opened while off": 0, "labels": ["Fluid", "Bars", "Off"], "checked": [False, True, False]}
    assert any(m.startswith("ready:") and m.endswith("overlay off") for m in messages)
    assert "overlay switched to bars from the tray" in messages
    (window,) = fake_main.windows.opened
    assert (window.renderer, window.notified, window.closed) == (STYLES["bars"], [RECORDING, RECORDING], True)
    assert json.loads((tmp_path / "tray_choices.json").read_text(encoding="utf-8")) == {"overlay": "bars"}


@pytest.mark.parametrize("available", [True, False])
def test_main_selects_claude_for_local_and_server_dictations_and_names_its_key(fake_main, monkeypatch, available):
    fake_main.config = replace(fake_main.config, postprocess=PostprocessConfig(provider="anthropic"))
    requested, clients = [], []

    def key(*, name):
        requested.append(name)
        return "test-key" if available else None

    def client(config, api_key):
        result = create_polisher(config, api_key, transport=httpx2.MockTransport(lambda r: httpx2.Response(500)))
        clients.append(result)
        return result

    monkeypatch.setattr(app, "load_api_key", key)
    monkeypatch.setattr(app, "create_polisher", client)
    servers = []

    def server(*args, **kwargs):
        result = DictationServer(*args, **kwargs)
        servers.append(result)
        return result

    monkeypatch.setattr(app, "DictationServer", server)

    def during(icon):
        item = icon.menu.items[0]
        assert item.text == "Post-process with Claude" + ("" if available else " (no ANTHROPIC_API_KEY)")
        assert item.enabled == item.checked == available
        local = fake_main.dictations[0]
        assert local["polisher"] is servers[0]._polish_client
        if available:
            assert isinstance(local["polisher"], ClaudeClient)
            item(icon)
            assert not local["polish_enabled"]() and not servers[0].polish_on()
        else:
            assert local["polisher"] is None

    assert fake_main.run(during=during) == 0
    assert requested == ["ANTHROPIC_API_KEY"]
    assert all(c._client.is_closed for c in clients)


def test_settings_is_the_default_tray_action_and_opens_the_window(fake_main):
    def during(icon):
        item = next(item for item in icon.menu.items if item.text == "Settings…")
        assert item.default
        item(icon)

    assert fake_main.run(during=during) == 0
    assert fake_main.launches == [("--settings",)]


def test_saved_settings_finish_dictation_then_restart_the_app(fake_main):
    def during(icon):
        assert app.signal_event(app.RELOAD_EVENT_NAME)
        assert fake_main.finished.wait(5)
        assert fake_main.stopped.wait(5)

    assert fake_main.run(during=during) == 0
    assert fake_main.launches == [("--notify-ready",)]


def test_a_second_launch_exits_and_the_running_app_says_it_is_already_running(fake_main):
    second: list[int] = []

    def during(icon: pystray.Icon) -> None:
        second.append(app.main())
        wait_until(lambda: fake_main.notifications, "the running app never showed a notification")

    assert fake_main.run(during=during) == 0
    assert second == [0]
    assert fake_main.notifications == [
        ("Parakeet Dictate is already running. Hold Right Ctrl to dictate.", "Parakeet Dictate")
    ]


def test_notify_ready_shows_one_notification_naming_the_configured_hotkey(fake_main):
    fake_main.config = Config(hotkey="shift_r", overlay="off", sounds=False)
    assert fake_main.run("--notify-ready") == 0
    assert fake_main.notifications == [("Ready. Hold Right Shift to dictate.", "Parakeet Dictate")]


def test_a_start_without_notify_ready_shows_no_notification(fake_main):
    assert fake_main.run() == 0
    assert fake_main.notifications == []


def test_a_saved_server_choice_serves_the_loaded_model_until_quit(fake_main, tmp_path, caplog):
    (tmp_path / "tray_choices.json").write_text('{"server": true}', encoding="utf-8")
    seen: dict[str, object] = {}

    def during(icon: pystray.Icon) -> None:
        (line,) = [r.getMessage() for r in caplog.records if r.getMessage().startswith("server for Mac listening")]
        seen["port"] = port = int(line.rsplit(":", 1)[1])
        token = (tmp_path / "server_token.txt").read_text(encoding="utf-8")
        response = httpx2.post(
            f"http://127.0.0.1:{port}/v1/audio/transcriptions",
            files={"file": ("a.wav", SECOND_OF_SILENCE, "audio/wav")},
            headers={"Authorization": f"Bearer {token}"},
            timeout=5,
        )
        seen["reply"] = response.json()
        seen["dictation shares the model"] = fake_main.dictations[0]["transcriber"].transcribe(None)
        seen["checked"] = server_items(icon)[0].checked

    assert fake_main.run(during=during) == 0
    assert seen["reply"] == {"text": "from the loaded model", "raw": "from the loaded model", "polished": False}
    assert (seen["dictation shares the model"], seen["checked"]) == ("from the loaded model", True)
    assert health_status(seen["port"], "any") is None, "the server outlived quit"
