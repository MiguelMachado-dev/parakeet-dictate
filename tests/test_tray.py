import json
import sys
import threading
import time
import uuid

import pystray
import pytest

from parakeet_dictate import app
from parakeet_dictate.app import ActiveOverlay, Tray, TrayChoices
from parakeet_dictate.config import Config
from parakeet_dictate.reducer import Cue, HotkeyDown, Idle, Mode, Recording, Transition
from parakeet_dictate.styles import STYLES

RECORDING = Transition(Idle(), Recording(Mode.HOLD, 0.0), HotkeyDown(0.0), ())


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


def test_the_overlay_menu_has_a_radio_item_per_style_and_off_that_switches_and_saves(tmp_path):
    path = tmp_path / "tray_choices.json"
    windows = Windows()
    overlay = ActiveOverlay("off", windows, FakeCues())
    tray = Tray(lambda: None, TrayChoices(Config(), path), overlay, glm_available=False)
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
    assert [item.text for item in tray.icon.menu.items if item.text] == ["Post-process with GLM (no ZAI_API_KEY)", "Overlay", "- - - -", "Open log", "Quit"]


def test_main_wires_the_tray_menu_to_the_overlay_and_the_dictation(tmp_path, monkeypatch, caplog):
    """Runs main() with a fake tray loop, transcriber and Dictation: no app mutex, no keyboard hook."""
    tag = uuid.uuid4().hex
    monkeypatch.setattr(sys, "argv", ["parakeet-dictate"])
    monkeypatch.setattr(app, "setup_logging", lambda: None)
    monkeypatch.setattr(app, "MUTEX_NAME", f"Local\\parakeet-dictate-test-{tag}")
    monkeypatch.setattr(app, "QUIT_EVENT_NAME", f"Local\\parakeet-dictate-test-quit-{tag}")
    monkeypatch.setattr(app, "TRAY_CHOICES_PATH", tmp_path / "tray_choices.json")
    monkeypatch.setattr(app, "load_config", lambda: Config(overlay="off", sounds=False))
    monkeypatch.setattr(app, "load_api_key", lambda: None)
    monkeypatch.setattr(app, "default_input_rate", lambda: 16_000)
    windows = Windows()
    monkeypatch.setattr(app, "Overlay", lambda renderer, levels: windows(renderer))

    class Transcriber:
        label = provider = "fake"

        def warm_up(self, rate: int) -> None:
            pass

    monkeypatch.setattr(app, "load_transcriber", lambda model_dir, provider: Transcriber())
    dictations: list[dict] = []

    class FakeDictation:
        def __init__(self, config, **parts) -> None:
            dictations.append(parts)

        def start(self) -> None:
            pass

        def stop(self) -> None:
            pass

    monkeypatch.setattr(app, "Dictation", FakeDictation)
    seen: dict[str, object] = {}

    def run(icon: pystray.Icon) -> None:
        deadline = time.perf_counter() + 5
        while not any(r.getMessage().startswith("ready:") for r in caplog.records):
            assert time.perf_counter() < deadline, "the loader never logged ready"
            time.sleep(0.01)
        on_transition = dictations[0]["on_transition"]
        on_transition(RECORDING)
        seen["opened while off"] = len(windows.opened)
        items = overlay_items(icon)
        seen["labels"] = [item.text for item in items]
        next(item for item in items if item.text == "Bars")(icon)
        seen["checked"] = [item.checked for item in items]
        on_transition(RECORDING)

    monkeypatch.setattr(pystray.Icon, "run", run)
    with caplog.at_level("INFO", logger="parakeet_dictate"):
        assert app.main() == 0
    messages = [r.getMessage() for r in caplog.records]

    assert seen == {"opened while off": 0, "labels": ["Fluid", "Bars", "Off"], "checked": [False, True, False]}
    assert any(m.startswith("ready:") and m.endswith("overlay off") for m in messages)
    assert "overlay switched to bars from the tray" in messages
    (window,) = windows.opened
    assert (window.renderer, window.notified, window.closed) == (STYLES["bars"], [RECORDING, RECORDING], True)
    assert json.loads((tmp_path / "tray_choices.json").read_text(encoding="utf-8")) == {"overlay": "bars"}
