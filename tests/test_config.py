import re

import pytest

from parakeet_dictate.app_rules import AppRule
from parakeet_dictate.config import (
    ROOT,
    Config,
    ConfigError,
    PostprocessConfig,
    ServerConfig,
    load_api_key,
    load_config,
)


def test_example_config_uncommented_uses_defaults_and_a_disabled_example_rule(tmp_path):
    example = (ROOT / "config.example.toml").read_text(encoding="utf-8")
    settings = re.findall(r"^# (\w+ = .+|\[\w+\]|\[\[\w+\]\])$", example, flags=re.MULTILINE)
    assert len(settings) == 26
    path = tmp_path / "config.toml"
    path.write_text("\n".join(settings), encoding="utf-8")
    assert load_config(path) == Config(app_rules=(
        AppRule("Translate to English", "Translate the transcript to English.", ("Slack.exe", "Discord.exe"), enabled=False),
    ))


def test_overrides_are_applied(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text(
        'hotkey = "f13"\nprovider = "cpu"\nsounds = false\noverlay = "bars"\nmodel_dir = "elsewhere"\n', encoding="utf-8"
    )
    config = load_config(path)
    assert (config.hotkey, config.provider, config.sounds, config.overlay, config.model_dir) == (
        "f13", "cpu", False, "bars", ROOT / "elsewhere"
    )


@pytest.mark.parametrize(
    ("hotkey", "label"),
    [
        ("ctrl_r", "Right Ctrl"),
        ("shift_r", "Right Shift"),
        ("alt_r", "Right Alt"),
        ("ctrl_l", "Left Ctrl"),
        ("cmd_r", "Right Windows"),
        ("alt_gr", "AltGr"),
        ("f13", "F13"),
        ("pause", "Pause"),
        ("scroll_lock", "Scroll Lock"),
    ],
)
def test_hotkey_label_names_the_key_as_people_do(hotkey, label):
    assert Config(hotkey=hotkey).hotkey_label == label


def test_overlay_shows_the_fluid_style_by_default():
    assert Config().overlay == "fluid"


def test_overlay_can_be_turned_off(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text('overlay = "off"\n', encoding="utf-8")
    assert load_config(path).overlay == "off"


def test_postprocess_table_overrides_only_the_settings_it_names(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text(
        'sounds = false\n[postprocess]\nenabled = false\nreasoning_effort = "high"\ntimeout_s = 5\n'
        'extra_instructions = "Names: João"\n',
        encoding="utf-8",
    )
    config = load_config(path)
    assert config.sounds is False
    assert config.postprocess == PostprocessConfig(
        enabled=False, reasoning_effort="high", timeout_s=5.0, extra_instructions="Names: João"
    )
    assert config.postprocess.model == "glm-5.3"


def test_server_table_follows_the_postprocess_table(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text('[postprocess]\nenabled = false\n[server]\nenabled = true\nbind = "127.0.0.1"\n', encoding="utf-8")
    config = load_config(path)
    assert config.postprocess == PostprocessConfig(enabled=False)
    assert config.server == ServerConfig(enabled=True, port=8765, bind="127.0.0.1")


@pytest.mark.parametrize(
    "line",
    ['hotke = "ctrl_r"', 'hotkey = "ctrl"', 'hotkey = "esc"', 'hotkey = "nope"', "tap_threshold_ms = true",
     'provider = "gpu"', 'sounds = "yes"', "overlay = 1", "overlay = true", 'overlay = "orb"','postprocess = "on"',
     '[postprocess]\nenable = true', '[postprocess]\nenabled = "yes"', '[postprocess]\nreasoning_effort = "medium"',
     "[postprocess]\ntimeout_s = 0", "[postprocess]\ntimeout_s = true", "[postprocess]\ntimeout_s = 600",
     '[postprocess]\nmodel = ""', '[postprocess]\nendpoint = "api.z.ai/v4"', "[postprocess]\nextra_instructions = 3",
     'server = "on"', "[server]\nenable = true", '[server]\nenabled = "yes"', "[server]\nport = 0",
     "[server]\nport = 70000", '[server]\nport = "8765"', '[server]\nbind = "localhost"', "[server]\nbind = 1",
     '[server]\nbind = "::"'],
)
def test_invalid_settings_are_rejected(tmp_path, line):
    path = tmp_path / "config.toml"
    path.write_text(line, encoding="utf-8")
    with pytest.raises(ConfigError):
        load_config(path)


def test_api_key_from_environment_wins_over_env_file(tmp_path, monkeypatch):
    env = tmp_path / ".env"
    env.write_text("OTHER=1\nZAI_API_KEY = 'from-file'\n", encoding="utf-8")
    monkeypatch.setenv("ZAI_API_KEY", "from-environment")
    assert load_api_key(env) == "from-environment"
    monkeypatch.delenv("ZAI_API_KEY")
    assert load_api_key(env) == "from-file"


def test_api_key_missing(tmp_path, monkeypatch):
    monkeypatch.delenv("ZAI_API_KEY", raising=False)
    assert load_api_key(tmp_path / ".env") is None
    (tmp_path / ".env").write_text("ZAI_API_KEY=\n", encoding="utf-8")
    assert load_api_key(tmp_path / ".env") is None


def test_anthropic_selects_its_own_defaults_and_key(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text('[postprocess]\nprovider = "anthropic"\n', encoding="utf-8")
    config = load_config(path).postprocess
    assert config == PostprocessConfig(provider="anthropic")
    assert config.model == "claude-haiku-5-5"
    assert config.endpoint == "https://api.anthropic.com/v1/messages"
    assert (config.label, config.api_key_name, config.thinking, config.effort) == ("Claude", "ANTHROPIC_API_KEY", False, "low")


def test_claude_settings_override_defaults(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text('[postprocess]\nprovider = "anthropic"\nmodel = "claude-sonnet-5-5"\n'
                    'endpoint = "http://localhost:5000/messages"\nthinking = true\neffort = "max"\n', encoding="utf-8")
    config = load_config(path).postprocess
    assert (config.model, config.endpoint, config.thinking, config.effort) == (
        "claude-sonnet-5-5", "http://localhost:5000/messages", True, "max",
    )


@pytest.mark.parametrize("setting", [
    'provider = "claude"', 'provider = true', 'thinking = "off"', 'effort = "extreme"',
    'provider = "anthropic"\neffort = "max"', 'provider = "anthropic"\neffort = "xhigh"',
])
def test_invalid_provider_settings_fail_before_a_request(tmp_path, setting):
    path = tmp_path / "config.toml"
    path.write_text(f"[postprocess]\n{setting}\n", encoding="utf-8")
    with pytest.raises(ConfigError):
        load_config(path)


def test_provider_keys_stay_separate_and_environment_takes_priority(tmp_path, monkeypatch):
    env = tmp_path / ".env"
    env.write_text('ZAI_API_KEY = "zai-file"\nANTHROPIC_API_KEY = "claude-file"\n', encoding="utf-8-sig")
    monkeypatch.setenv("ZAI_API_KEY", "zai-env")
    monkeypatch.setenv("ANTHROPIC_API_KEY", " claude-env ")
    assert load_api_key(env, name="ANTHROPIC_API_KEY") == "claude-env"
    assert load_api_key(env) == "zai-env"
    monkeypatch.delenv("ANTHROPIC_API_KEY")
    assert load_api_key(env, name="ANTHROPIC_API_KEY") == "claude-file"
    env.write_text('ZAI_API_KEY = "zai-file"\n', encoding="utf-8")
    assert load_api_key(env, name="ANTHROPIC_API_KEY") is None


def test_app_rules_load_in_order_beside_postprocess_and_server_settings(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text('[postprocess]\nprovider = "anthropic"\nenabled = false\n[server]\nport = 8766\n'
                    '[[app_rules]]\nname = "English"\napps = ["Slack.exe", "outlook.exe"]\n'
                    'instructions = "Translate to English."\n'
                    '[[app_rules]]\nname = "Casual"\napps = ["Slack.exe"]\n'
                    'instructions = "Keep it casual."\nenabled = false\n', encoding="utf-8")
    config = load_config(path)
    assert config.postprocess == PostprocessConfig(provider="anthropic", enabled=False)
    assert config.server.port == 8766
    assert config.app_rules == (
        AppRule("English", "Translate to English.", ("Slack.exe", "outlook.exe")),
        AppRule("Casual", "Keep it casual.", ("Slack.exe",), enabled=False),
    )


@pytest.mark.parametrize("rule", [
    'app_rules = "on"', 'app_rules = ["Slack.exe"]',
    '[[app_rules]]\nname = "English"',
    '[[app_rules]]\nname = ""\ninstructions = "Translate."',
    '[[app_rules]]\nname = "English"\ninstructions = 42',
    '[[app_rules]]\nname = "English"\ninstructions = "Translate."\nenabled = "yes"',
    '[[app_rules]]\nname = "English"\ninstructions = "Translate."\napp = ["Slack.exe"]',
    *[f'[[app_rules]]\nname = "English"\ninstructions = "Translate."\napps = {apps}'
      for apps in ['"Slack.exe"', '[1]', '["Slack"]', '["C:/Slack.exe"]', '["*.exe"]', '[" Slack.exe"]']],
])
def test_invalid_app_rules_are_rejected(tmp_path, rule):
    path = tmp_path / "config.toml"
    path.write_text(rule, encoding="utf-8")
    with pytest.raises(ConfigError):
        load_config(path)
