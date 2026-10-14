import time
import tkinter as tk
from dataclasses import replace

import pytest

from parakeet_dictate import settings_window
from parakeet_dictate.config import PostprocessConfig
from parakeet_dictate.settings_store import KEY_NAMES, SettingsStore
from parakeet_dictate.settings_window import SettingsWindow


@pytest.fixture
def window(tmp_path, monkeypatch, capfd):
    for name in KEY_NAMES:
        monkeypatch.delenv(name, raising=False)
    # On this Windows Tcl build, pytest's fd redirection breaks Tcl's library reads
    # between interpreters. Keep native descriptors stable for the whole window lifetime.
    with capfd.disabled():
        root = tk.Tk()
        root.withdraw()
        applied = []
        view = SettingsWindow(root, SettingsStore(tmp_path), on_apply=lambda: applied.append(True) or "Applied in test.")
        view.applied = applied
        root.update()
        yield view
        if not view.closed:
            view.dirty = False
            view.close()


def choose_claude(window):
    window.vars["postprocess.provider"].set("Anthropic (Claude)")
    window._change_provider()


def test_opening_settings_does_not_write_files_or_apply_changes(window):
    assert [window.notebook.tab(tab, "text") for tab in window.notebook.tabs()] == [
        "General", "Post-processing", "App rules", "Server for Mac",
    ]
    assert str(window.save_button["state"]) == "disabled"
    assert window.key_entry["show"] == "•"
    assert not window.applied
    assert list(window.store.root.iterdir()) == []


def test_provider_switching_keeps_keys_separate_and_limits_thinking_off_effort(window):
    window.keys["ZAI_API_KEY"].set("zai-test")
    choose_claude(window)
    window.keys["ANTHROPIC_API_KEY"].set("claude-test")
    assert window.vars["postprocess.model"].get() == "claude-haiku-5-5"
    assert window.vars["postprocess.endpoint"].get() == "https://api.anthropic.com/v1/messages"
    window.vars["postprocess.thinking"].set(True)
    window._thinking_changed()
    window.vars["postprocess.effort"].set("Max")
    window.vars["postprocess.thinking"].set(False)
    window._thinking_changed()
    assert window.vars["postprocess.effort"].get() == "High"
    assert tuple(window.effort_box["values"]) == ("Low", "Medium", "High")
    window.vars["postprocess.provider"].set("Z.ai (GLM)")
    window._change_provider()
    assert window.vars["postprocess.model"].get() == "glm-5.3"
    choose_claude(window)
    assert window.save()
    saved = window.store.load()
    assert saved.keys == {"ZAI_API_KEY": "zai-test", "ANTHROPIC_API_KEY": "claude-test"}
    assert saved.config.postprocess.provider == "anthropic" and saved.config.postprocess.effort == "high"
    assert window.applied == [True]


def test_rules_can_be_added_edited_disabled_reordered_removed_and_saved(window):
    rules = window.rules
    rules.add_rule("English", "Translate to English.")
    rules.add_app("Slack.exe")
    rules.add_app("SLACK.EXE")
    assert rules.get_rules()[0].apps == ("Slack.exe",)
    rules.add_rule("Casual", "Keep it casual.")
    rules.add_app("Discord.exe")
    rules.enabled.set(False)
    rules.move_rule(-1)
    assert [rule.name for rule in rules.get_rules()] == ["Casual", "English"]
    assert window.save()
    assert [rule.enabled for rule in window.store.load().config.app_rules] == [False, True]
    rules.remove_rule()
    assert window.save()
    assert [rule.name for rule in window.store.load().config.app_rules] == ["English"]
    window.reload()
    assert window.rules.get_rules() == window.store.load().config.app_rules


def test_invalid_values_or_external_edits_do_not_apply(window):
    window.vars["server.port"].set("not a port")
    assert not window.save()
    assert "server.port" in window.status.get() and not window.applied
    assert list(window.store.root.iterdir()) == []
    window.vars["server.port"].set("8765")
    (window.store.root / "tray_choices.json").write_text('{"overlay":"bars"}')
    assert not window.save()
    assert "Reload" in window.status.get() and not window.applied
    assert (window.store.root / "tray_choices.json").read_text() == '{"overlay":"bars"}'


def test_closing_or_declining_reload_preserves_unsaved_disk_state(window, monkeypatch):
    window.vars["sounds"].set(False)
    monkeypatch.setattr(settings_window.messagebox, "askyesno", lambda *args, **kwargs: False)
    window.reload()
    window.close()
    assert not window.closed and not window.vars["sounds"].get()
    monkeypatch.setattr(settings_window.messagebox, "askyesno", lambda *args, **kwargs: True)
    window.close()
    assert window.closed and not window.applied and list(window.store.root.iterdir()) == []


def test_key_from_environment_is_masked_and_read_only(window, monkeypatch):
    monkeypatch.setenv("ZAI_API_KEY", "environment-test")
    window.reload()
    assert window.key_entry["show"] == "•"
    assert str(window.key_entry["state"]) == "readonly"
    assert "environment variable" in window.key_hint["text"]


def test_test_provider_uses_the_draft_without_saving_and_keeps_the_window_responsive(window, monkeypatch):
    calls = []
    class Client:
        def polish(self, raw):
            calls.append(raw)
        def close(self):
            calls.append("closed")

    monkeypatch.setattr(settings_window, "create_polisher", lambda config, key: Client())
    choose_claude(window)
    window.keys["ANTHROPIC_API_KEY"].set("test-key")
    window.test_provider()
    deadline = time.monotonic() + 3
    while str(window.test_button["state"]) == "disabled" and time.monotonic() < deadline:
        window.root.update()
        time.sleep(0.01)
    assert "Claude replied" in window.test_status.get()
    assert calls == ["um hello this is a dictation test", "closed"]
    assert list(window.store.root.iterdir()) == [] and not window.applied


def test_loading_a_saved_provider_and_rules_back_into_widgets(window):
    snapshot = window.store.load()
    config = replace(snapshot.config, overlay="bars", postprocess=PostprocessConfig(provider="anthropic", thinking=True, effort="max"))
    window.store.save(snapshot, config, snapshot.keys)
    window.reload()
    assert window.vars["overlay"].get() == "Bars"
    assert window.vars["postprocess.provider"].get() == "Anthropic (Claude)"
    assert window.vars["postprocess.thinking"].get() and window.vars["postprocess.effort"].get() == "Max"
