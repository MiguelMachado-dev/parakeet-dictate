from dataclasses import replace

import pytest

from parakeet_dictate.config import ConfigError, load_config, parse_config
from parakeet_dictate.dictionary import PersonalDictionary
from parakeet_dictate.settings_store import SettingsStore, config_values


def test_dictionary_round_trips_through_settings_and_toml_without_losing_comments(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text('# Keep me\nhotkey = "f13" # spare key\n[dictionary]\n# My terms\nwords = []\n')
    store = SettingsStore(tmp_path)
    before = store.load()
    dictionary = PersonalDictionary.from_text("PI\nharness\nJoão", "PY => PI\nmeu link => https://example.com/$1")
    config = replace(before.config, dictionary=dictionary)
    after = store.save(before, config, before.keys)
    assert after.config.dictionary == load_config(path).dictionary == dictionary
    assert parse_config(config_values(config)) == config
    assert "# Keep me" in path.read_text() and "# spare key" in path.read_text() and "# My terms" in path.read_text()
    empty = replace(after.config, dictionary=PersonalDictionary())
    assert store.save(after, empty, after.keys).config.dictionary == PersonalDictionary()
    assert load_config(path).dictionary == PersonalDictionary()


@pytest.mark.parametrize("dictionary", [
    [], {"unknown": []}, {"words": "PI"}, {"words": [1]}, {"words": ["PI", "pi"]},
    {"replacements": {}}, {"replacements": ["PY"]}, {"replacements": [{"source": "PY"}]},
    {"replacements": [{"source": "PY", "target": 5}]},
    {"replacements": [{"source": "PY", "target": "PI"}, {"source": "py", "target": "Python"}]},
])
def test_invalid_dictionary_is_rejected_without_echoing_terms(dictionary):
    with pytest.raises(ConfigError):
        parse_config({"dictionary": dictionary})
