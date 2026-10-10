import re

import pytest

from parakeet_dictate.config import ROOT, Config, ConfigError, PostprocessConfig, load_api_key, load_config


def test_example_config_uncommented_matches_defaults(tmp_path):
    example = (ROOT / "config.example.toml").read_text(encoding="utf-8")
    settings = re.findall(r"^# (\w+ = .+|\[\w+\])$", example, flags=re.MULTILINE)
    assert len(settings) == 14
    path = tmp_path / "config.toml"
    path.write_text("\n".join(settings), encoding="utf-8")
    assert load_config(path) == Config()


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


@pytest.mark.parametrize(
    "line",
    ['hotke = "ctrl_r"', 'hotkey = "ctrl"', 'hotkey = "esc"', 'hotkey = "nope"', "tap_threshold_ms = true",
     'provider = "gpu"', 'sounds = "yes"', "overlay = 1", "overlay = true", 'overlay = "orb"','postprocess = "on"',
     '[postprocess]\nenable = true', '[postprocess]\nenabled = "yes"', '[postprocess]\nreasoning_effort = "medium"',
     "[postprocess]\ntimeout_s = 0", "[postprocess]\ntimeout_s = true", "[postprocess]\ntimeout_s = 600",
     '[postprocess]\nmodel = ""', '[postprocess]\nendpoint = "api.z.ai/v4"', "[postprocess]\nextra_instructions = 3"],
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
