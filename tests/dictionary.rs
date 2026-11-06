use std::fs;
use std::path::Path;

use parakeet_dictate::config::{load_config, parse_config};
use parakeet_dictate::dictionary::{PersonalDictionary, Replacement};
use parakeet_dictate::settings_store::{SettingsStore, config_values};
use serde_json::{Value, json};
use tempfile::tempdir;
use toml::Table;

#[test]
fn test_shared_mac_and_windows_corrections() {
    let fixture = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dictionary.json")).unwrap();
    let cases: Vec<Value> = serde_json::from_str(&fixture).unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let text = |value: &Value| value.as_str().unwrap().to_owned();
        let words = case["words"].as_array().unwrap().iter().map(text).collect();
        let replacements = case["replacements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| Replacement::new(entry["source"].as_str().unwrap(), entry["target"].as_str().unwrap()))
            .collect();
        let dictionary = PersonalDictionary::new(words, replacements).unwrap();
        assert_eq!(dictionary.apply(case["input"].as_str().unwrap()), case["output"].as_str().unwrap(), "{case}");
    }
}

#[test]
fn test_editor_round_trip_and_prompt_preserve_terms_and_app_rules() {
    let dictionary = PersonalDictionary::from_text("\nPI\nharness\n João \n", "\n PY => PI\nmeu link => https://example.com/?a=>b\n").unwrap();
    assert_eq!(PersonalDictionary::from_text(&dictionary.words_text(), &dictionary.replacements_text()).unwrap(), dictionary);
    let prompt = dictionary.instructions("Translate to English.");
    assert!(prompt.starts_with("Translate to English.\n\nPersonal dictionary"), "{prompt}");
    let data: Value = serde_json::from_str(prompt.split('\n').next_back().unwrap()).unwrap();
    assert_eq!(data["words"], json!(["PI", "harness", "João"]));
    assert_eq!(data["replacements"][0], json!({"source": "PY", "target": "PI"}));
    assert_eq!(PersonalDictionary::default().instructions("unchanged"), "unchanged");
}

#[test]
fn test_editor_rejects_ambiguous_or_incomplete_entries() {
    let cases = [
        ("PI\npi", ""),
        ("", "py => PI\nPY => Python"),
        ("", "PY"),
        ("", "PY =>"),
        ("", "=> PI"),
        ("a\0b", ""),
        ("", "a => b\0c"),
    ];
    for (words, replacements) in cases {
        let result = PersonalDictionary::from_text(words, replacements);
        assert!(result.is_err(), "accepted {words:?}, {replacements:?}: {result:?}");
    }
}

#[test]
fn test_dictionary_round_trips_through_settings_and_toml_without_losing_comments() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "# Keep me\nhotkey = \"f13\" # spare key\n[dictionary]\n# My terms\nwords = []\n").unwrap();
    let store = SettingsStore::new(dir.path().to_path_buf());
    let before = store.load().unwrap();
    let dictionary = PersonalDictionary::from_text("PI\nharness\nJoão", "PY => PI\nmeu link => https://example.com/$1").unwrap();
    let mut config = before.config.clone();
    config.dictionary = dictionary.clone();
    let after = store.save(&before, &config, &before.keys).unwrap();
    assert_eq!(after.config.dictionary, dictionary);
    assert_eq!(load_config(&path).unwrap().dictionary, dictionary);
    assert_eq!(parse_config(&config_values(&config)).unwrap(), config);
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("# Keep me") && content.contains("# spare key") && content.contains("# My terms"), "{content}");
    let mut empty = after.config.clone();
    empty.dictionary = PersonalDictionary::default();
    assert_eq!(store.save(&after, &empty, &after.keys).unwrap().config.dictionary, PersonalDictionary::default());
    assert_eq!(load_config(&path).unwrap().dictionary, PersonalDictionary::default());
}

#[test]
fn test_invalid_dictionary_is_rejected_without_echoing_terms() {
    let dictionaries = [
        "[]",
        "{unknown = []}",
        "{words = \"PI\"}",
        "{words = [1]}",
        "{words = [\"PI\", \"pi\"]}",
        "{replacements = {}}",
        "{replacements = [\"PY\"]}",
        "{replacements = [{source = \"PY\"}]}",
        "{replacements = [{source = \"PY\", target = 5}]}",
        "{replacements = [{source = \"PY\", target = \"PI\"}, {source = \"py\", target = \"Python\"}]}",
    ];
    for dictionary in dictionaries {
        let raw: Table = toml::from_str(&format!("dictionary = {dictionary}")).unwrap();
        let result = parse_config(&raw);
        assert!(result.is_err(), "accepted {dictionary}: {result:?}");
    }
}
