import pytest

from parakeet_dictate.app import PostprocessChoice


@pytest.mark.parametrize("default", [True, False])
def test_config_default_applies_until_the_tray_is_toggled(tmp_path, default):
    path = tmp_path / "postprocess.json"
    assert PostprocessChoice(default, path).on is default
    assert not path.exists()


def test_tray_choice_survives_a_restart_and_wins_over_the_config_default(tmp_path):
    path = tmp_path / "postprocess.json"
    PostprocessChoice(True, path).toggle()
    assert PostprocessChoice(True, path).on is False
    restarted = PostprocessChoice(False, path)
    restarted.toggle()
    assert PostprocessChoice(False, path).on is True


@pytest.mark.parametrize("content", ["", "not json", "[]", '{"enabled": "yes"}', '{"on": false}'])
def test_unreadable_choice_falls_back_to_the_config_default(tmp_path, content):
    path = tmp_path / "postprocess.json"
    path.write_text(content, encoding="utf-8")
    assert PostprocessChoice(True, path).on is True
    assert PostprocessChoice(False, path).on is False
