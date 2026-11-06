use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use parakeet_dictate::app_rules::AppRule;
use parakeet_dictate::config::{PolishProvider, PostprocessConfig, load_api_key, load_config};
use parakeet_dictate::settings_store::{KEY_NAMES, SettingsError, SettingsStore, write_file};
use parakeet_dictate::tray_choices::TrayChoices;
use serde_json::json;
use tempfile::{TempDir, tempdir};

static ENV: Mutex<()> = Mutex::new(());

/// The `store` fixture: an empty folder, with neither key in the environment until the test sets one.
struct Fixture {
    store: SettingsStore,
    dir: TempDir,
    saved: Vec<(&'static str, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let lock = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = KEY_NAMES.iter().map(|name| (*name, env::var_os(name))).collect();
        for name in KEY_NAMES {
            // SAFETY: ENV serialises every test in this binary.
            unsafe { env::remove_var(name) };
        }
        let dir = tempdir().unwrap();
        Self { store: SettingsStore::new(dir.path().to_path_buf()), dir, saved, _lock: lock }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root().join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            match value {
                // SAFETY: the lock is still held; it is released after this drop returns.
                Some(value) => unsafe { env::set_var(name, value) },
                None => unsafe { env::remove_var(name) },
            }
        }
    }
}

fn entries(root: &Path) -> Vec<String> {
    fs::read_dir(root).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect()
}

fn with_key(keys: &BTreeMap<String, String>, name: &str, value: &str) -> BTreeMap<String, String> {
    let mut keys = keys.clone();
    keys.insert(name.to_owned(), value.to_owned());
    keys
}

#[test]
fn test_saved_tray_choices_are_the_values_shown_in_settings() {
    let fixture = Fixture::new();
    fs::write(fixture.path("tray_choices.json"), r#"{"overlay":"bars","postprocess":false,"server":true}"#).unwrap();
    let config = fixture.store.load().unwrap().config;
    assert!(config.overlay == "bars" && !config.postprocess.enabled && config.server.enabled);
}

#[test]
fn test_settings_and_tray_use_the_same_valid_saved_choices() {
    let contents: [&[u8]; 5] = [
        b"not json",
        b"[]",
        br#"{"overlay":"invalid","postprocess":false}"#,
        br#"{"postprocess":"yes","server":1}"#,
        b"\xef\xbb\xbf{\"overlay\":\"bars\"}",
    ];
    for content in contents {
        let fixture = Fixture::new();
        let path = fixture.path("tray_choices.json");
        fs::write(&path, content).unwrap();
        let config = load_config(&fixture.path("config.toml")).unwrap();
        let tray = TrayChoices::new(&config, path.clone());
        let snapshot = fixture.store.load().unwrap();
        assert_eq!(snapshot.config.overlay, tray.overlay());
        assert_eq!(snapshot.config.postprocess.enabled, tray.postprocess());
        assert_eq!(snapshot.config.server.enabled, tray.server());
        assert_eq!(fs::read(&path).unwrap(), content);
        let saved = fixture.store.save(&snapshot, &snapshot.config, &snapshot.keys).unwrap();
        assert_eq!(saved.config, snapshot.config);
    }
}

#[test]
fn test_tray_change_during_deferred_reload_keeps_the_settings_just_saved() {
    let fixture = Fixture::new();
    let snapshot = fixture.store.load().unwrap();
    let tray = TrayChoices::new(&snapshot.config, fixture.path("tray_choices.json"));
    tray.choose("postprocess", json!(true));
    let snapshot = fixture.store.load().unwrap();
    let mut updated = snapshot.config.clone();
    updated.postprocess.enabled = false;
    updated.server.enabled = true;
    fixture.store.save(&snapshot, &updated, &snapshot.keys).unwrap();
    tray.choose("overlay", json!("off"));
    let after = fixture.store.load().unwrap().config;
    assert!(!after.postprocess.enabled && after.server.enabled && after.overlay == "off");
}

#[test]
fn test_save_round_trips_rules_and_provider_and_preserves_comments_and_existing_keys() {
    let fixture = Fixture::new();
    fs::write(fixture.path("config.toml"), "# Personal settings\nhotkey = \"f13\" # spare key\n").unwrap();
    fs::write(fixture.path(".env"), b"\xef\xbb\xbf# My keys\r\nOTHER=keep-this\r\nZAI_API_KEY=\"zai-test\"\r\n").unwrap();
    let snapshot = fixture.store.load().unwrap();
    let mut config = snapshot.config.clone();
    config.overlay = "off".to_owned();
    config.postprocess = PostprocessConfig::for_provider(PolishProvider::Anthropic);
    config.app_rules = vec![
        AppRule::new("English \"chat\"", "Translate to English.\nKeep João's name.", &["Slack.exe"]),
        AppRule::new("Formal", "Be formal.", &["OUTLOOK.EXE"]).disabled(),
    ];
    let saved = fixture.store.save(&snapshot, &config, &with_key(&snapshot.keys, "ANTHROPIC_API_KEY", "claude-test")).unwrap();
    assert_eq!(saved.config, config);
    assert_eq!(load_config(&fixture.path("config.toml")).unwrap(), config);
    let content = fs::read_to_string(fixture.path("config.toml")).unwrap();
    assert!(content.contains("# Personal settings") && content.contains("# spare key"), "{content}");
    assert!(!content.contains("zai-test") && !content.contains("claude-test"), "{content}");
    let env_path = fixture.path(".env");
    assert_eq!(load_api_key(&env_path, "ZAI_API_KEY").as_deref(), Some("zai-test"));
    assert_eq!(load_api_key(&env_path, "ANTHROPIC_API_KEY").as_deref(), Some("claude-test"));
    assert!(fs::read_to_string(&env_path).unwrap().contains("OTHER=keep-this"));
    let choices = TrayChoices::new(&config, fixture.path("tray_choices.json"));
    assert!(choices.overlay() == "off" && choices.postprocess() && !choices.server());
    assert!(!format!("{saved:?}").contains("claude-test"));
}

#[test]
fn test_unchanged_key_file_is_preserved_byte_for_byte() {
    let fixture = Fixture::new();
    let data = b"\xef\xbb\xbf# existing key\r\nZAI_API_KEY = \"test\"\r\n";
    fs::write(fixture.path(".env"), data).unwrap();
    let snapshot = fixture.store.load().unwrap();
    let mut config = snapshot.config.clone();
    config.sounds = false;
    fixture.store.save(&snapshot, &config, &snapshot.keys).unwrap();
    assert_eq!(fs::read(fixture.path(".env")).unwrap(), data);
}

#[test]
fn test_environment_key_is_read_only_and_never_copied_to_the_file() {
    let fixture = Fixture::new();
    fs::write(fixture.path(".env"), "ZAI_API_KEY=file-key\n").unwrap();
    // SAFETY: ENV serialises every test in this binary, and the fixture removes this again.
    unsafe { env::set_var("ZAI_API_KEY", "environment-key") };
    let snapshot = fixture.store.load().unwrap();
    assert_eq!(snapshot.environment_keys, BTreeSet::from(["ZAI_API_KEY".to_owned()]));
    let saved = fixture.store.save(&snapshot, &snapshot.config, &with_key(&snapshot.keys, "ZAI_API_KEY", "attempted-edit")).unwrap();
    assert_eq!(saved.keys["ZAI_API_KEY"], "environment-key");
    assert_eq!(fs::read_to_string(fixture.path(".env")).unwrap(), "ZAI_API_KEY=file-key\n");
}

#[test]
fn test_external_edits_are_not_overwritten() {
    for name in ["config.toml", ".env", "tray_choices.json"] {
        let fixture = Fixture::new();
        let snapshot = fixture.store.load().unwrap();
        fs::write(fixture.path(name), "changed elsewhere").unwrap();
        let error = fixture.store.save(&snapshot, &snapshot.config, &snapshot.keys).unwrap_err();
        assert!(matches!(error, SettingsError::Conflict), "{name}: {error:?}");
        assert!(error.to_string().contains("Reload"), "{error}");
        assert_eq!(fs::read_to_string(fixture.path(name)).unwrap(), "changed elsewhere");
    }
}

#[test]
fn test_invalid_settings_or_keys_write_nothing() {
    let fixture = Fixture::new();
    let snapshot = fixture.store.load().unwrap();
    let mut config = snapshot.config.clone();
    config.hotkey = "esc".to_owned();
    let error = fixture.store.save(&snapshot, &config, &snapshot.keys).unwrap_err();
    assert!(matches!(error, SettingsError::Invalid(_)), "{error:?}");
    let keys = with_key(&snapshot.keys, "ANTHROPIC_API_KEY", "first\nsecond");
    let error = fixture.store.save(&snapshot, &snapshot.config, &keys).unwrap_err();
    assert!(matches!(error, SettingsError::Invalid(_)), "{error:?}");
    assert_eq!(entries(fixture.root()), Vec::<String>::new());
}

fn fail_key_file(root: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    if name == ".env" {
        return Err(io::Error::other("simulated write failure"));
    }
    write_file(root, name, data)
}

#[test]
fn test_failed_save_restores_already_written_files() {
    let fixture = Fixture::new();
    fs::write(fixture.path("config.toml"), "hotkey = \"f13\"\n").unwrap();
    let store = SettingsStore::with_writer(fixture.root().to_path_buf(), fail_key_file);
    let snapshot = store.load().unwrap();
    let mut config = snapshot.config.clone();
    config.sounds = false;
    let error = store.save(&snapshot, &config, &with_key(&snapshot.keys, "ANTHROPIC_API_KEY", "test")).unwrap_err();
    assert!(matches!(error, SettingsError::Io(_)), "{error:?}");
    assert_eq!(store.load().unwrap().files, snapshot.files);
    assert!(!entries(fixture.root()).iter().any(|name| name.ends_with(".tmp")));
}
