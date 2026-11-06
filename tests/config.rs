use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use parakeet_dictate::app_rules::AppRule;
use parakeet_dictate::config::{Config, PolishProvider, PostprocessConfig, ServerConfig, load_api_key, load_config};
use parakeet_dictate::paths;
use parakeet_dictate::text::is_word_char;
use tempfile::{TempDir, tempdir};

const ZAI: &str = "ZAI_API_KEY";
const ANTHROPIC: &str = "ANTHROPIC_API_KEY";

static ENV: Mutex<()> = Mutex::new(());

/// The environment for one test: holds the lock and puts both key variables back when dropped.
struct KeyEnvironment {
    _lock: MutexGuard<'static, ()>,
    saved: [(&'static str, Option<OsString>); 2],
}

impl KeyEnvironment {
    fn take() -> Self {
        let lock = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = [ZAI, ANTHROPIC].map(|name| (name, env::var_os(name)));
        let environment = Self { _lock: lock, saved };
        environment.remove(ZAI);
        environment.remove(ANTHROPIC);
        environment
    }

    fn set(&self, name: &str, value: &str) {
        // SAFETY: the lock serialises every test in this binary that touches the environment.
        unsafe { env::set_var(name, value) };
    }

    fn remove(&self, name: &str) {
        // SAFETY: the lock serialises every test in this binary that touches the environment.
        unsafe { env::remove_var(name) };
    }
}

impl Drop for KeyEnvironment {
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

fn config_file(content: &str) -> (TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, content).unwrap();
    (dir, path)
}

fn loaded(content: &str) -> Config {
    let (_dir, path) = config_file(content);
    load_config(&path).unwrap()
}

fn assert_rejected(content: &str) {
    let (_dir, path) = config_file(content);
    let result = load_config(&path);
    assert!(result.is_err(), "accepted {content:?}: {result:?}");
}

/// `^# (\w+ = .+|\[\w+\]|\[\[\w+\]\])$`, the settings the example shows commented out.
fn commented_setting(line: &str) -> Option<&str> {
    let is_word = |text: &str| !text.is_empty() && text.chars().all(is_word_char);
    let setting = line.strip_prefix("# ")?;
    let assignment = setting.split_once(" = ").is_some_and(|(name, value)| is_word(name) && !value.is_empty());
    let table = setting.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')).is_some_and(is_word);
    let tables = setting.strip_prefix("[[").and_then(|rest| rest.strip_suffix("]]")).is_some_and(is_word);
    (assignment || table || tables).then_some(setting)
}

#[test]
fn test_example_config_uncommented_uses_defaults_and_a_disabled_example_rule() {
    let example = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.toml")).unwrap();
    let settings: Vec<&str> = example.lines().filter_map(commented_setting).collect();
    assert_eq!(settings.len(), 29);
    let expected = Config {
        app_rules: vec![
            AppRule::new("Translate to English", "Translate the transcript to English.", &["Slack.exe", "Discord.exe"]).disabled(),
        ],
        ..Config::default()
    };
    assert_eq!(loaded(&settings.join("\n")), expected);
}

#[test]
fn test_overrides_are_applied() {
    let config = loaded("hotkey = \"f13\"\nprovider = \"cpu\"\nsounds = false\noverlay = \"bars\"\nmodel_dir = \"elsewhere\"\n");
    assert_eq!(config.hotkey, "f13");
    assert_eq!(config.provider.name(), "cpu");
    assert!(!config.sounds);
    assert_eq!(config.overlay, "bars");
    assert_eq!(config.model_dir, paths::root().join("elsewhere"));
}

#[test]
fn test_hotkey_label_names_the_key_as_people_do() {
    let cases = [
        ("ctrl_r", "Right Ctrl"),
        ("shift_r", "Right Shift"),
        ("alt_r", "Right Alt"),
        ("ctrl_l", "Left Ctrl"),
        ("cmd_r", "Right Windows"),
        ("alt_gr", "AltGr"),
        ("f13", "F13"),
        ("pause", "Pause"),
        ("scroll_lock", "Scroll Lock"),
    ];
    for (hotkey, label) in cases {
        let config = Config { hotkey: hotkey.to_owned(), ..Config::default() };
        assert_eq!(config.hotkey_label(), label);
    }
}

#[test]
fn test_overlay_shows_the_fluid_style_by_default() {
    assert_eq!(Config::default().overlay, "fluid");
}

#[test]
fn test_overlay_can_be_turned_off() {
    assert_eq!(loaded("overlay = \"off\"\n").overlay, "off");
}

#[test]
fn test_postprocess_table_overrides_only_the_settings_it_names() {
    let config = loaded(
        "sounds = false\n[postprocess]\nenabled = false\nreasoning_effort = \"high\"\ntimeout_s = 5\n\
         extra_instructions = \"Names: João\"\n",
    );
    assert!(!config.sounds);
    let expected = PostprocessConfig {
        enabled: false,
        reasoning_effort: "high".to_owned(),
        timeout_s: 5.0,
        extra_instructions: "Names: João".to_owned(),
        ..PostprocessConfig::default()
    };
    assert_eq!(config.postprocess, expected);
    assert_eq!(config.postprocess.model, "glm-5.3");
}

#[test]
fn test_server_table_follows_the_postprocess_table() {
    let config = loaded("[postprocess]\nenabled = false\n[server]\nenabled = true\nbind = \"127.0.0.1\"\n");
    assert_eq!(config.postprocess, PostprocessConfig { enabled: false, ..PostprocessConfig::default() });
    assert_eq!(config.server, ServerConfig { enabled: true, port: 8765, bind: "127.0.0.1".to_owned() });
}

#[test]
fn test_invalid_settings_are_rejected() {
    let lines = [
        "hotke = \"ctrl_r\"",
        "hotkey = \"ctrl\"",
        "hotkey = \"esc\"",
        "hotkey = \"nope\"",
        "tap_threshold_ms = true",
        "provider = \"gpu\"",
        "sounds = \"yes\"",
        "overlay = 1",
        "overlay = true",
        "overlay = \"orb\"",
        "postprocess = \"on\"",
        "[postprocess]\nenable = true",
        "[postprocess]\nenabled = \"yes\"",
        "[postprocess]\nreasoning_effort = \"medium\"",
        "[postprocess]\ntimeout_s = 0",
        "[postprocess]\ntimeout_s = true",
        "[postprocess]\ntimeout_s = 600",
        "[postprocess]\nmodel = \"\"",
        "[postprocess]\nendpoint = \"api.z.ai/v4\"",
        "[postprocess]\nextra_instructions = 3",
        "server = \"on\"",
        "[server]\nenable = true",
        "[server]\nenabled = \"yes\"",
        "[server]\nport = 0",
        "[server]\nport = 70000",
        "[server]\nport = \"8765\"",
        "[server]\nbind = \"localhost\"",
        "[server]\nbind = 1",
        "[server]\nbind = \"::\"",
    ];
    for line in lines {
        assert_rejected(line);
    }
}

#[test]
fn test_api_key_from_environment_wins_over_env_file() {
    let environment = KeyEnvironment::take();
    let dir = tempdir().unwrap();
    let env_path = dir.path().join(".env");
    fs::write(&env_path, "OTHER=1\nZAI_API_KEY = 'from-file'\n").unwrap();
    environment.set(ZAI, "from-environment");
    assert_eq!(load_api_key(&env_path, ZAI).as_deref(), Some("from-environment"));
    environment.remove(ZAI);
    assert_eq!(load_api_key(&env_path, ZAI).as_deref(), Some("from-file"));
}

#[test]
fn test_api_key_missing() {
    let _environment = KeyEnvironment::take();
    let dir = tempdir().unwrap();
    let env_path = dir.path().join(".env");
    assert_eq!(load_api_key(&env_path, ZAI), None);
    fs::write(&env_path, "ZAI_API_KEY=\n").unwrap();
    assert_eq!(load_api_key(&env_path, ZAI), None);
}

#[test]
fn test_anthropic_selects_its_own_defaults_and_key() {
    let config = loaded("[postprocess]\nprovider = \"anthropic\"\n").postprocess;
    assert_eq!(config, PostprocessConfig::for_provider(PolishProvider::Anthropic));
    assert_eq!(config.model, "claude-haiku-5-5");
    assert_eq!(config.endpoint, "https://api.anthropic.com/v1/messages");
    assert_eq!(config.label(), "Claude");
    assert_eq!(config.api_key_name(), "ANTHROPIC_API_KEY");
    assert!(!config.thinking);
    assert_eq!(config.effort, "low");
}

#[test]
fn test_claude_settings_override_defaults() {
    let config = loaded(
        "[postprocess]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-5-5\"\n\
         endpoint = \"http://localhost:5000/messages\"\nthinking = true\neffort = \"max\"\n",
    )
    .postprocess;
    assert_eq!(config.model, "claude-sonnet-5-5");
    assert_eq!(config.endpoint, "http://localhost:5000/messages");
    assert!(config.thinking);
    assert_eq!(config.effort, "max");
}

#[test]
fn test_invalid_provider_settings_fail_before_a_request() {
    let settings = [
        "provider = \"claude\"",
        "provider = true",
        "thinking = \"off\"",
        "effort = \"extreme\"",
        "provider = \"anthropic\"\neffort = \"max\"",
        "provider = \"anthropic\"\neffort = \"xhigh\"",
    ];
    for setting in settings {
        assert_rejected(&format!("[postprocess]\n{setting}\n"));
    }
}

#[test]
fn test_provider_keys_stay_separate_and_environment_takes_priority() {
    let environment = KeyEnvironment::take();
    let dir = tempdir().unwrap();
    let env_path = dir.path().join(".env");
    fs::write(&env_path, "\u{feff}ZAI_API_KEY = \"zai-file\"\nANTHROPIC_API_KEY = \"claude-file\"\n").unwrap();
    environment.set(ZAI, "zai-env");
    environment.set(ANTHROPIC, " claude-env ");
    assert_eq!(load_api_key(&env_path, ANTHROPIC).as_deref(), Some("claude-env"));
    assert_eq!(load_api_key(&env_path, ZAI).as_deref(), Some("zai-env"));
    environment.remove(ANTHROPIC);
    assert_eq!(load_api_key(&env_path, ANTHROPIC).as_deref(), Some("claude-file"));
    fs::write(&env_path, "ZAI_API_KEY = \"zai-file\"\n").unwrap();
    assert_eq!(load_api_key(&env_path, ANTHROPIC), None);
}

#[test]
fn test_app_rules_load_in_order_beside_postprocess_and_server_settings() {
    let config = loaded(
        "[postprocess]\nprovider = \"anthropic\"\nenabled = false\n[server]\nport = 8766\n\
         [[app_rules]]\nname = \"English\"\napps = [\"Slack.exe\", \"outlook.exe\"]\n\
         instructions = \"Translate to English.\"\n\
         [[app_rules]]\nname = \"Casual\"\napps = [\"Slack.exe\"]\n\
         instructions = \"Keep it casual.\"\nenabled = false\n",
    );
    let postprocess = PostprocessConfig { enabled: false, ..PostprocessConfig::for_provider(PolishProvider::Anthropic) };
    assert_eq!(config.postprocess, postprocess);
    assert_eq!(config.server.port, 8766);
    assert_eq!(
        config.app_rules,
        [
            AppRule::new("English", "Translate to English.", &["Slack.exe", "outlook.exe"]),
            AppRule::new("Casual", "Keep it casual.", &["Slack.exe"]).disabled(),
        ],
    );
}

#[test]
fn test_invalid_app_rules_are_rejected() {
    let rules = [
        "app_rules = \"on\"",
        "app_rules = [\"Slack.exe\"]",
        "[[app_rules]]\nname = \"English\"",
        "[[app_rules]]\nname = \"\"\ninstructions = \"Translate.\"",
        "[[app_rules]]\nname = \"English\"\ninstructions = 42",
        "[[app_rules]]\nname = \"English\"\ninstructions = \"Translate.\"\nenabled = \"yes\"",
        "[[app_rules]]\nname = \"English\"\ninstructions = \"Translate.\"\napp = [\"Slack.exe\"]",
    ];
    for rule in rules {
        assert_rejected(rule);
    }
    for apps in ["\"Slack.exe\"", "[1]", "[\"Slack\"]", "[\"C:/Slack.exe\"]", "[\"*.exe\"]", "[\" Slack.exe\"]"] {
        assert_rejected(&format!("[[app_rules]]\nname = \"English\"\ninstructions = \"Translate.\"\napps = {apps}"));
    }
}
