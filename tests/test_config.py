import re

import pytest

from parakeet_dictate.config import ROOT, Config, ConfigError, load_config


def test_example_config_uncommented_matches_defaults(tmp_path):
    example = (ROOT / "config.example.toml").read_text(encoding="utf-8")
    settings = re.findall(r"^# (\w+ = .+)$", example, flags=re.MULTILINE)
    assert len(settings) == 6
    path = tmp_path / "config.toml"
    path.write_text("\n".join(settings), encoding="utf-8")
    assert load_config(path) == Config()


def test_overrides_are_applied(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text('hotkey = "f13"\nprovider = "cpu"\nsounds = false\nmodel_dir = "elsewhere"\n', encoding="utf-8")
    config = load_config(path)
    assert (config.hotkey, config.provider, config.sounds, config.model_dir) == ("f13", "cpu", False, ROOT / "elsewhere")


@pytest.mark.parametrize(
    "line",
    ['hotke = "ctrl_r"', 'hotkey = "ctrl"', 'hotkey = "esc"', 'hotkey = "nope"', "tap_threshold_ms = true",
     'provider = "gpu"', 'sounds = "yes"'],
)
def test_invalid_settings_are_rejected(tmp_path, line):
    path = tmp_path / "config.toml"
    path.write_text(line, encoding="utf-8")
    with pytest.raises(ConfigError):
        load_config(path)
