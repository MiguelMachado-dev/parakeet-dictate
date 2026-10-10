import json
import logging
import os

import pytest

from parakeet_dictate.app import TrayChoices
from parakeet_dictate.config import Config, PostprocessConfig


def config(postprocess: bool = True, overlay: str = "fluid") -> Config:
    return Config(overlay=overlay, postprocess=PostprocessConfig(enabled=postprocess))


@pytest.mark.parametrize(("postprocess", "overlay"), [(True, "fluid"), (False, "off"), (True, "bars")])
def test_config_values_apply_until_the_tray_chooses(tmp_path, postprocess, overlay):
    path = tmp_path / "tray_choices.json"
    choices = TrayChoices(config(postprocess, overlay), path)
    assert (choices.postprocess, choices.overlay) == (postprocess, overlay)
    assert not path.exists()


def test_a_saved_choice_wins_over_the_config_after_a_restart(tmp_path):
    path = tmp_path / "tray_choices.json"
    TrayChoices(config(True, "fluid"), path).choose("overlay", "off")
    TrayChoices(config(True, "fluid"), path).choose("postprocess", False)
    restarted = TrayChoices(config(True, "bars"), path)
    assert (restarted.postprocess, restarted.overlay) == (False, "off")
    assert json.loads(path.read_text(encoding="utf-8")) == {"overlay": "off", "postprocess": False}


def test_choosing_one_key_leaves_the_other_on_its_config_value(tmp_path):
    path = tmp_path / "tray_choices.json"
    TrayChoices(config(True, "fluid"), path).choose("overlay", "bars")
    assert TrayChoices(config(False, "fluid"), path).postprocess is False
    assert json.loads(path.read_text(encoding="utf-8")) == {"overlay": "bars"}


@pytest.mark.parametrize(
    "content",
    ["", "not json", "[]", '{"postprocess": "yes", "overlay": "orb"}', '{"postprocess": 1, "overlay": null}'],
)
def test_an_invalid_saved_value_falls_back_to_the_config_and_warns_once(tmp_path, caplog, content):
    path = tmp_path / "tray_choices.json"
    path.write_text(content, encoding="utf-8")
    with caplog.at_level(logging.WARNING):
        choices = TrayChoices(config(False, "bars"), path)
        warnings = len(caplog.records)
        assert (choices.postprocess, choices.overlay) == (False, "bars")
        assert (choices.postprocess, choices.overlay) == (False, "bars")
    assert 1 <= warnings == len(caplog.records) <= 2


def test_one_invalid_key_keeps_the_valid_one(tmp_path):
    path = tmp_path / "tray_choices.json"
    path.write_text('{"postprocess": false, "overlay": "orb"}', encoding="utf-8")
    choices = TrayChoices(config(True, "fluid"), path)
    assert (choices.postprocess, choices.overlay) == (False, "fluid")


def test_a_failed_save_leaves_the_previous_file_whole(tmp_path, monkeypatch):
    path = tmp_path / "tray_choices.json"
    choices = TrayChoices(config(), path)
    choices.choose("overlay", "bars")
    before = path.read_text(encoding="utf-8")

    def interrupted(src, dst):
        raise OSError("disk full")

    monkeypatch.setattr(os, "replace", interrupted)
    choices.choose("postprocess", False)
    assert path.read_text(encoding="utf-8") == before
    assert TrayChoices(config(), path).overlay == "bars"
