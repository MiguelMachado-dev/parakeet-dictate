import json
from pathlib import Path

import pytest

from parakeet_dictate.dictionary import PersonalDictionary, Replacement


@pytest.mark.parametrize("case", json.loads((Path(__file__).parent / "fixtures/dictionary.json").read_text(encoding="utf-8")))
def test_shared_mac_and_windows_corrections(case):
    dictionary = PersonalDictionary(tuple(case["words"]), tuple(Replacement(**entry) for entry in case["replacements"]))
    assert dictionary.apply(case["input"]) == case["output"]


def test_editor_round_trip_and_prompt_preserve_terms_and_app_rules():
    dictionary = PersonalDictionary.from_text("\nPI\nharness\n João \n", "\n PY => PI\nmeu link => https://example.com/?a=>b\n")
    assert PersonalDictionary.from_text(dictionary.words_text, dictionary.replacements_text) == dictionary
    prompt = dictionary.instructions("Translate to English.")
    assert prompt.startswith("Translate to English.\n\nPersonal dictionary")
    data = json.loads(prompt.split("\n")[-1])
    assert data["words"] == ["PI", "harness", "João"]
    assert data["replacements"][0] == {"source": "PY", "target": "PI"}
    assert PersonalDictionary().instructions("unchanged") == "unchanged"


@pytest.mark.parametrize("words,replacements", [
    ("PI\npi", ""), ("", "py => PI\nPY => Python"), ("", "PY"), ("", "PY =>"),
    ("", "=> PI"), ("a\0b", ""), ("", "a => b\0c"),
])
def test_editor_rejects_ambiguous_or_incomplete_entries(words, replacements):
    with pytest.raises(ValueError):
        PersonalDictionary.from_text(words, replacements)
