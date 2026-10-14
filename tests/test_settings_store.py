from dataclasses import replace

import pytest

from parakeet_dictate.app_rules import AppRule
from parakeet_dictate.config import ConfigError, PostprocessConfig, load_api_key, load_config
from parakeet_dictate.settings_store import KEY_NAMES, SettingsConflict, SettingsStore
from parakeet_dictate.tray_choices import TrayChoices


@pytest.fixture
def store(tmp_path, monkeypatch):
    for name in KEY_NAMES:
        monkeypatch.delenv(name, raising=False)
    return SettingsStore(tmp_path)


def test_saved_tray_choices_are_the_values_shown_in_settings(store):
    (store.root / "tray_choices.json").write_text('{"overlay":"bars","postprocess":false,"server":true}')
    config = store.load().config
    assert config.overlay == "bars" and not config.postprocess.enabled and config.server.enabled


@pytest.mark.parametrize("content", [
    b"not json", b"[]", b'{"overlay":"invalid","postprocess":false}',
    b'{"postprocess":"yes","server":1}', b'\xef\xbb\xbf{"overlay":"bars"}',
])
def test_settings_and_tray_use_the_same_valid_saved_choices(store, content):
    path = store.root / "tray_choices.json"
    path.write_bytes(content)
    config = load_config(store.root / "config.toml")
    tray = TrayChoices(config, path)
    snapshot = store.load()
    assert (snapshot.config.overlay, snapshot.config.postprocess.enabled, snapshot.config.server.enabled) == (
        tray.overlay, tray.postprocess, tray.server,
    )
    assert path.read_bytes() == content
    saved = store.save(snapshot, snapshot.config, snapshot.keys)
    assert saved.config == snapshot.config


def test_tray_change_during_deferred_reload_keeps_the_settings_just_saved(store):
    snapshot = store.load()
    tray = TrayChoices(snapshot.config, store.root / "tray_choices.json")
    tray.choose("postprocess", True)
    snapshot = store.load()
    updated = replace(snapshot.config, postprocess=replace(snapshot.config.postprocess, enabled=False),
                      server=replace(snapshot.config.server, enabled=True))
    store.save(snapshot, updated, snapshot.keys)
    tray.choose("overlay", "off")
    after = store.load().config
    assert not after.postprocess.enabled and after.server.enabled and after.overlay == "off"


def test_save_round_trips_rules_and_provider_and_preserves_comments_and_existing_keys(store):
    (store.root / "config.toml").write_text('# Personal settings\nhotkey = "f13" # spare key\n', encoding="utf-8")
    env = b'\xef\xbb\xbf# My keys\r\nOTHER=keep-this\r\nZAI_API_KEY="zai-test"\r\n'
    (store.root / ".env").write_bytes(env)
    snapshot = store.load()
    config = replace(snapshot.config, overlay="off", postprocess=PostprocessConfig(provider="anthropic"), app_rules=(
        AppRule('English "chat"', "Translate to English.\nKeep João's name.", ("Slack.exe",)),
        AppRule("Formal", "Be formal.", ("OUTLOOK.EXE",), enabled=False),
    ))
    saved = store.save(snapshot, config, {**snapshot.keys, "ANTHROPIC_API_KEY": "claude-test"})
    assert saved.config == config == load_config(store.root / "config.toml")
    content = (store.root / "config.toml").read_text(encoding="utf-8")
    assert "# Personal settings" in content and "# spare key" in content
    assert "zai-test" not in content and "claude-test" not in content
    assert load_api_key(store.root / ".env") == "zai-test"
    assert load_api_key(store.root / ".env", name="ANTHROPIC_API_KEY") == "claude-test"
    assert "OTHER=keep-this" in (store.root / ".env").read_text(encoding="utf-8")
    choices = TrayChoices(config, store.root / "tray_choices.json")
    assert choices.overlay == "off" and choices.postprocess and not choices.server
    assert "claude-test" not in repr(saved)


def test_unchanged_key_file_is_preserved_byte_for_byte(store):
    data = b'\xef\xbb\xbf# existing key\r\nZAI_API_KEY = "test"\r\n'
    (store.root / ".env").write_bytes(data)
    snapshot = store.load()
    store.save(snapshot, replace(snapshot.config, sounds=False), snapshot.keys)
    assert (store.root / ".env").read_bytes() == data


def test_environment_key_is_read_only_and_never_copied_to_the_file(store, monkeypatch):
    (store.root / ".env").write_text("ZAI_API_KEY=file-key\n")
    monkeypatch.setenv("ZAI_API_KEY", "environment-key")
    snapshot = store.load()
    assert snapshot.environment_keys == {"ZAI_API_KEY"}
    saved = store.save(snapshot, snapshot.config, {**snapshot.keys, "ZAI_API_KEY": "attempted-edit"})
    assert saved.keys["ZAI_API_KEY"] == "environment-key"
    assert (store.root / ".env").read_text() == "ZAI_API_KEY=file-key\n"


@pytest.mark.parametrize("name", ["config.toml", ".env", "tray_choices.json"])
def test_external_edits_are_not_overwritten(store, name):
    snapshot = store.load()
    (store.root / name).write_text("changed elsewhere")
    with pytest.raises(SettingsConflict, match="Reload"):
        store.save(snapshot, snapshot.config, snapshot.keys)
    assert (store.root / name).read_text() == "changed elsewhere"


def test_invalid_settings_or_keys_write_nothing(store):
    snapshot = store.load()
    with pytest.raises(ConfigError):
        store.save(snapshot, replace(snapshot.config, hotkey="esc"), snapshot.keys)
    with pytest.raises(ConfigError):
        store.save(snapshot, snapshot.config, {**snapshot.keys, "ANTHROPIC_API_KEY": "first\nsecond"})
    assert list(store.root.iterdir()) == []


def test_failed_save_restores_already_written_files(store, monkeypatch):
    (store.root / "config.toml").write_text('hotkey = "f13"\n')
    snapshot = store.load()
    write = store._write

    def fail_key_file(name, data):
        if name == ".env":
            raise OSError("simulated write failure")
        write(name, data)

    monkeypatch.setattr(store, "_write", fail_key_file)
    with pytest.raises(OSError):
        store.save(snapshot, replace(snapshot.config, sounds=False), {**snapshot.keys, "ANTHROPIC_API_KEY": "test"})
    assert store.load().files == snapshot.files
    assert not list(store.root.glob("*.tmp"))
