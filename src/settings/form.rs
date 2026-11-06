//! What the Settings window edits, without the window: the draft, its validation, saving, and every rule
//! about which choice allows which. The controls only copy values in and out of `Editor`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Instant;

use toml::Value;

use crate::app_rules::AppRule;
use crate::config::{ConfigError, Engine, PolishProvider, PostprocessConfig, is_executable_name, parse_config};
use crate::dictionary::{PersonalDictionary, Replacement};
use crate::hotkeys;
use crate::polish::{Polish, create_polisher};
use crate::settings_store::{SettingsError, SettingsSnapshot, SettingsStore, config_values};
use crate::text::{casefold, short_float, split_lines};

pub const PROVIDERS: [(&str, PolishProvider); 2] = [("Z.ai (GLM)", PolishProvider::Zai), ("Anthropic (Claude)", PolishProvider::Anthropic)];
pub const ENGINES: [(&str, Engine); 2] = [("NVIDIA GPU (CUDA)", Engine::Cuda), ("CPU", Engine::Cpu)];
pub const EFFORTS: [(&str, &str); 5] = [("Low", "low"), ("Medium", "medium"), ("High", "high"), ("Extra high", "xhigh"), ("Max", "max")];
pub const OVERLAYS: [&str; 3] = ["Fluid", "Bars", "Off"];
pub const PRESETS: [(&str, &str); 4] = [
    ("Translate to English", "Translate the transcript to English."),
    ("Casual", "Make the tone casual and friendly, as in a chat message."),
    ("Formal", "Make the tone formal and polite."),
    ("Blank rule", ""),
];
const SAMPLE: &str = "O PY é o harness.";
const TEST_SAMPLE: &str = "um hello this is a dictation test";
const FIRST_HOTKEYS: [&str; 11] = ["ctrl_r", "ctrl_l", "shift_r", "shift_l", "alt_r", "alt_l", "cmd_r", "cmd_l", "pause", "scroll_lock", "insert"];

/// Exactly what the controls show: labels as displayed, numbers as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fields {
    pub hotkey: String,
    pub engine: String,
    pub overlay: String,
    pub sounds: bool,
    pub append_space: bool,
    pub tap_threshold_ms: String,
    pub model_dir: String,
    pub postprocess_enabled: bool,
    pub polish_provider: String,
    pub polish_model: String,
    pub thinking: bool,
    pub effort: String,
    pub timeout_s: String,
    pub extra_instructions: String,
    pub endpoint: String,
    pub server_enabled: bool,
    pub server_port: String,
    pub server_bind: String,
    pub words: String,
    pub sample: String,
    pub keys: BTreeMap<String, String>,
    /// Original and replacement, never empty: the page always offers a row to type in.
    pub replacements: Vec<(String, String)>,
    pub rules: Vec<AppRule>,
    pub selected_rule: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Success,
    Failure,
}

type OnApply = Box<dyn Fn() -> std::io::Result<String>>;
type CreateClient = Arc<dyn Fn(&PostprocessConfig, &str) -> Arc<dyn Polish> + Send + Sync>;

struct ProviderTest {
    results: Receiver<String>,
    started: Instant,
    label: &'static str,
    timeout_s: f64,
}

pub struct Editor {
    pub store: SettingsStore,
    pub snapshot: SettingsSnapshot,
    pub fields: Fields,
    pub dirty: bool,
    pub status: String,
    pub tone: Tone,
    pub test_status: String,
    on_apply: OnApply,
    create_client: CreateClient,
    hotkeys: Vec<(String, String)>,
    provider: PolishProvider,
    provider_values: [PostprocessConfig; 2],
    test: Option<ProviderTest>,
}

fn slot(provider: PolishProvider) -> usize {
    usize::from(provider == PolishProvider::Anthropic)
}

fn effort_label(config: &PostprocessConfig) -> &'static str {
    let effort = if config.provider == PolishProvider::Anthropic { &config.effort } else { &config.reasoning_effort };
    EFFORTS.iter().find(|(_, value)| value == effort).map_or("Low", |(label, _)| label)
}

fn hotkey_choices(current: &str) -> Vec<(String, String)> {
    let numbered = (1..=24).map(|number| format!("f{number}"));
    let keys = FIRST_HOTKEYS.iter().map(|key| (*key).to_owned()).chain(numbered).chain(Some(current.to_owned()));
    let mut choices: Vec<(String, String)> = Vec::new();
    for key in keys {
        let label = hotkeys::label_of(&key);
        match choices.iter_mut().find(|(known, _)| *known == label) {
            Some(choice) => choice.1 = key,
            None => choices.push((label, key)),
        }
    }
    choices
}

pub fn rule_title(rule: &AppRule) -> String {
    let name = if rule.name.is_empty() { "Untitled" } else { &rule.name };
    format!("{name}{}", if rule.enabled { "" } else { " (off)" })
}

impl Editor {
    pub fn open(store: SettingsStore, on_apply: OnApply) -> Result<Self, SettingsError> {
        let create_client: CreateClient = Arc::new(|config, key| Arc::new(create_polisher(config, key, None)));
        Self::with_client(store, on_apply, create_client)
    }

    pub fn with_client(store: SettingsStore, on_apply: OnApply, create_client: CreateClient) -> Result<Self, SettingsError> {
        let snapshot = store.load()?;
        let mut editor = Self {
            store,
            snapshot,
            fields: Fields::default(),
            dirty: false,
            status: String::new(),
            tone: Tone::Neutral,
            test_status: String::new(),
            on_apply,
            create_client,
            hotkeys: Vec::new(),
            provider: PolishProvider::Zai,
            provider_values: [PostprocessConfig::for_provider(PolishProvider::Zai), PostprocessConfig::for_provider(PolishProvider::Anthropic)],
            test: None,
        };
        editor.load();
        Ok(editor)
    }

    /// Show the snapshot, with nothing left to save.
    fn load(&mut self) {
        let config = &self.snapshot.config;
        let values = config_values(config);
        let text = |value: Option<&Value>| value.and_then(Value::as_str).unwrap_or_default().to_owned();
        self.hotkeys = hotkey_choices(&config.hotkey);
        let post = &config.postprocess;
        self.provider = post.provider;
        self.provider_values = [PostprocessConfig::for_provider(PolishProvider::Zai), PostprocessConfig::for_provider(PolishProvider::Anthropic)];
        self.provider_values[slot(post.provider)] = post.clone();
        let label_of = |list: &[(&'static str, PolishProvider)]| list.iter().find(|(_, value)| *value == post.provider).map_or("", |(label, _)| label);
        let engine = ENGINES.iter().find(|(_, value)| *value == config.provider).map_or("", |(label, _)| label);
        let overlay = OVERLAYS.iter().find(|label| label.to_lowercase() == config.overlay).unwrap_or(&"Off");
        let replacements: Vec<_> = config.dictionary.replacements().iter().map(|entry| (entry.source.clone(), entry.target.clone())).collect();
        let selected_rule = (!config.app_rules.is_empty()).then_some(0);
        self.fields = Fields {
            hotkey: config.hotkey_label(),
            engine: engine.to_owned(),
            overlay: (*overlay).to_owned(),
            sounds: config.sounds,
            append_space: config.append_space,
            tap_threshold_ms: config.tap_threshold_ms.to_string(),
            model_dir: text(values.get("model_dir")),
            postprocess_enabled: post.enabled,
            polish_provider: label_of(&PROVIDERS).to_owned(),
            polish_model: post.model.clone(),
            thinking: post.thinking,
            effort: effort_label(post).to_owned(),
            timeout_s: short_float(post.timeout_s),
            extra_instructions: post.extra_instructions.clone(),
            endpoint: post.endpoint.clone(),
            server_enabled: config.server.enabled,
            server_port: config.server.port.to_string(),
            server_bind: config.server.bind.clone(),
            words: config.dictionary.words_text(),
            sample: SAMPLE.to_owned(),
            keys: self.snapshot.keys.clone(),
            replacements: if replacements.is_empty() { vec![(String::new(), String::new())] } else { replacements },
            rules: config.app_rules.clone(),
            selected_rule,
        };
        self.limit_effort();
        self.test_status.clear();
        self.dirty = false;
        self.status = "Changes take effect after Save and apply.".to_owned();
        self.tone = Tone::Neutral;
    }

    pub fn hotkey_labels(&self) -> Vec<String> {
        self.hotkeys.iter().map(|(label, _)| label.clone()).collect()
    }

    pub fn changed(&mut self) {
        self.dirty = true;
        self.status = "You have unsaved changes.".to_owned();
        self.tone = Tone::Neutral;
    }

    pub fn error(&mut self, text: &str) {
        text.clone_into(&mut self.status);
        self.tone = Tone::Failure;
    }

    pub fn provider(&self) -> PolishProvider {
        self.provider
    }

    pub fn api_key_name(&self) -> &'static str {
        PostprocessConfig::for_provider(self.provider).api_key_name()
    }

    pub fn key_label(&self) -> &'static str {
        if self.provider == PolishProvider::Anthropic { "Anthropic API key" } else { "Z.ai API key" }
    }

    /// The key comes from the environment, so editing it here would change nothing.
    pub fn key_is_managed(&self) -> bool {
        self.snapshot.environment_keys.contains(self.api_key_name())
    }

    pub fn key_hint(&self) -> String {
        if self.key_is_managed() {
            format!("Set by the {} environment variable.", self.api_key_name())
        } else {
            "Saved on this PC. Each provider keeps its own key.".to_owned()
        }
    }

    pub fn model_choices(&self) -> &'static [&'static str] {
        if self.provider == PolishProvider::Anthropic { &["claude-haiku-5-5", "claude-sonnet-5-5"] } else { &["glm-5.3"] }
    }

    pub fn allowed_efforts(&self) -> &'static [&'static str] {
        match (self.provider, self.fields.thinking) {
            (PolishProvider::Zai, _) => &["Low", "High", "Max"],
            (PolishProvider::Anthropic, true) => &["Low", "Medium", "High", "Extra high", "Max"],
            (PolishProvider::Anthropic, false) => &["Low", "Medium", "High"],
        }
    }

    /// An effort the current provider and thinking choice do not allow falls back to High.
    pub fn limit_effort(&mut self) {
        if !self.allowed_efforts().contains(&self.fields.effort.as_str()) {
            "High".clone_into(&mut self.fields.effort);
        }
    }

    pub fn change_provider(&mut self, label: &str) {
        let Some((_, provider)) = PROVIDERS.iter().find(|(known, _)| *known == label) else {
            return;
        };
        let effort = self.effort_value();
        let fields = &self.fields;
        let old = &mut self.provider_values[slot(self.provider)];
        old.model.clone_from(&fields.polish_model);
        old.endpoint.clone_from(&fields.endpoint);
        old.thinking = fields.thinking;
        if self.provider == PolishProvider::Anthropic {
            old.effort = effort;
        } else {
            old.reasoning_effort = effort;
        }
        self.provider = *provider;
        label.clone_into(&mut self.fields.polish_provider);
        let new = &self.provider_values[slot(self.provider)];
        self.fields.polish_model.clone_from(&new.model);
        self.fields.endpoint.clone_from(&new.endpoint);
        self.fields.thinking = new.thinking;
        effort_label(new).clone_into(&mut self.fields.effort);
        self.limit_effort();
        self.test_status.clear();
        self.changed();
    }

    fn effort_value(&self) -> String {
        let effort = EFFORTS.iter().find(|(label, _)| *label == self.fields.effort);
        effort.map_or("high", |(_, value)| value).to_owned()
    }

    fn draft_replacements(&self) -> Result<Vec<Replacement>, String> {
        let mut entries = Vec::new();
        for (index, (source, target)) in self.fields.replacements.iter().enumerate() {
            let (source, target) = (source.trim(), target.trim());
            if source.is_empty() && target.is_empty() {
                continue;
            }
            if source.is_empty() || target.is_empty() {
                return Err(format!("Replacement row {}: enter both original and replacement.", index + 1));
            }
            entries.push(Replacement::new(source, target));
        }
        Ok(entries)
    }

    fn draft_dictionary(&self) -> Result<PersonalDictionary, ConfigError> {
        let words = PersonalDictionary::from_text(&self.fields.words, "").map_err(ConfigError)?.words().to_vec();
        PersonalDictionary::new(words, self.draft_replacements().map_err(ConfigError)?).map_err(ConfigError)
    }

    pub fn dictionary_preview(&self) -> String {
        self.draft_dictionary().map_or_else(|e| e.to_string(), |dictionary| dictionary.apply(&self.fields.sample))
    }

    pub fn dictionary_example(&mut self) {
        let lines = split_lines(&self.fields.words);
        let existing: Vec<String> = lines.iter().map(|line| casefold(line.trim())).collect();
        for word in ["PI", "harness"] {
            if !existing.contains(&casefold(word)) {
                if !self.fields.words.is_empty() {
                    self.fields.words.push('\n');
                }
                self.fields.words.push_str(word);
            }
        }
        let rows = &mut self.fields.replacements;
        if !rows.iter().any(|(source, _)| casefold(source.trim()) == "py") {
            let example = ("PY".to_owned(), "PI".to_owned());
            match rows.iter_mut().find(|(source, target)| source.is_empty() && target.is_empty()) {
                Some(row) => *row = example,
                None => rows.push(example),
            }
        }
        self.changed();
    }

    /// With a single empty row there is nothing to remove, so the page hides its button.
    pub fn rows_can_be_removed(&self) -> bool {
        let rows = &self.fields.replacements;
        rows.len() > 1 || rows.iter().any(|(source, target)| !source.is_empty() || !target.is_empty())
    }

    pub fn add_row(&mut self, after: usize) {
        let at = (after + 1).min(self.fields.replacements.len());
        self.fields.replacements.insert(at, (String::new(), String::new()));
        self.changed();
    }

    pub fn remove_row(&mut self, index: usize) {
        self.fields.replacements.remove(index);
        if self.fields.replacements.is_empty() {
            self.fields.replacements.push((String::new(), String::new()));
        }
        self.changed();
    }

    pub fn selected_rule(&mut self) -> Option<&mut AppRule> {
        let index = self.fields.selected_rule?;
        self.fields.rules.get_mut(index)
    }

    pub fn add_rule(&mut self, name: &str, instructions: &str) {
        self.fields.rules.push(AppRule::new(name, instructions, &[]));
        self.fields.selected_rule = Some(self.fields.rules.len() - 1);
        self.changed();
    }

    pub fn remove_rule(&mut self) {
        let Some(index) = self.fields.selected_rule else {
            return;
        };
        self.fields.rules.remove(index);
        self.fields.selected_rule = (!self.fields.rules.is_empty()).then(|| index.min(self.fields.rules.len() - 1));
        self.changed();
    }

    pub fn move_rule(&mut self, offset: isize) {
        let Some(index) = self.fields.selected_rule else {
            return;
        };
        let target = index.wrapping_add_signed(offset);
        if target >= self.fields.rules.len() {
            return;
        }
        self.fields.rules.swap(index, target);
        self.fields.selected_rule = Some(target);
        self.changed();
    }

    /// False when `name` is no executable name; the status then says what to enter.
    pub fn add_app(&mut self, name: &str) -> bool {
        let name = name.trim();
        if self.fields.selected_rule.is_none() {
            return false;
        }
        if !is_executable_name(name) {
            self.error("Enter an executable name such as Slack.exe, or choose an app with Browse.");
            return false;
        }
        let Some(rule) = self.selected_rule() else {
            return false;
        };
        let known = rule.apps.iter().any(|app| casefold(app) == casefold(name));
        if !known {
            rule.apps.push(name.to_owned());
            self.changed();
        }
        true
    }

    pub fn remove_app(&mut self, index: usize) {
        if let Some(rule) = self.selected_rule()
            && index < rule.apps.len()
        {
            rule.apps.remove(index);
            self.changed();
        }
    }

    fn number<T: std::str::FromStr>(name: &str, value: &str) -> Result<T, ConfigError> {
        value.trim().parse().map_err(|_| ConfigError(format!("{name} must be a number.")))
    }

    pub fn draft(&self) -> Result<crate::config::Config, ConfigError> {
        let fields = &self.fields;
        let tap_threshold_ms: i64 = Self::number("tap_threshold_ms", &fields.tap_threshold_ms)?;
        let timeout_s: f64 = Self::number("postprocess.timeout_s", &fields.timeout_s)?;
        let port: i64 = Self::number("server.port", &fields.server_port)?;

        let mut base = self.snapshot.config.clone();
        base.app_rules.clone_from(&fields.rules);
        base.dictionary = self.draft_dictionary()?;
        let mut values = config_values(&base);
        let hotkey = self.hotkeys.iter().find(|(label, _)| *label == fields.hotkey).map_or_else(|| fields.hotkey.clone(), |(_, key)| key.clone());
        let engine = ENGINES.iter().find(|(label, _)| *label == fields.engine).map_or("", |(_, value)| value.name());
        values.insert("hotkey".into(), Value::String(hotkey));
        values.insert("provider".into(), Value::String(engine.to_owned()));
        values.insert("overlay".into(), Value::String(fields.overlay.to_lowercase()));
        values.insert("sounds".into(), Value::Boolean(fields.sounds));
        values.insert("append_space".into(), Value::Boolean(fields.append_space));
        values.insert("tap_threshold_ms".into(), Value::Integer(tap_threshold_ms));
        values.insert("model_dir".into(), Value::String(fields.model_dir.clone()));

        let postprocess = values["postprocess"].as_table_mut().expect("config_values writes a table");
        let effort_key = if self.provider == PolishProvider::Anthropic { "effort" } else { "reasoning_effort" };
        postprocess.insert("enabled".into(), Value::Boolean(fields.postprocess_enabled));
        postprocess.insert("provider".into(), Value::String(self.provider.name().to_owned()));
        postprocess.insert("model".into(), Value::String(fields.polish_model.clone()));
        postprocess.insert("endpoint".into(), Value::String(fields.endpoint.clone()));
        postprocess.insert("thinking".into(), Value::Boolean(fields.thinking));
        postprocess.insert(effort_key.into(), Value::String(self.effort_value()));
        postprocess.insert("timeout_s".into(), Value::Float(timeout_s));
        postprocess.insert("extra_instructions".into(), Value::String(fields.extra_instructions.clone()));

        let server = values["server"].as_table_mut().expect("config_values writes a table");
        server.insert("enabled".into(), Value::Boolean(fields.server_enabled));
        server.insert("port".into(), Value::Integer(port));
        server.insert("bind".into(), Value::String(fields.server_bind.clone()));
        parse_config(&values)
    }

    fn try_save(&mut self) -> Result<(), SettingsError> {
        let config = self.draft()?;
        let folder_changed = config.model_dir != self.snapshot.config.model_dir;
        if folder_changed && !is_model_folder(&config.model_dir) {
            return Err(ConfigError("Choose a Parakeet model folder containing config.json.".to_owned()).into());
        }
        self.snapshot = self.store.save(&self.snapshot, &config, &self.fields.keys)?;
        Ok(())
    }

    pub fn save(&mut self) -> bool {
        if let Err(e) = self.try_save() {
            self.error(&e.to_string());
            return false;
        }
        self.dirty = false;
        match (self.on_apply)() {
            Ok(status) => {
                self.status = status;
                self.tone = Tone::Success;
            }
            Err(_) => self.error("Saved, but the app could not reload. Start Parakeet Dictate from the Desktop."),
        }
        true
    }

    pub fn reload(&mut self) {
        match self.store.load() {
            Ok(snapshot) => {
                self.snapshot = snapshot;
                self.load();
            }
            Err(e) => self.error(&e.to_string()),
        }
    }

    pub fn testing(&self) -> bool {
        self.test.is_some()
    }

    /// Try the draft's provider with a short sample, on another thread. Nothing is saved.
    pub fn test_provider(&mut self) -> bool {
        let config = match self.draft() {
            Ok(config) => config.postprocess,
            Err(e) => {
                self.error(&e.to_string());
                return false;
            }
        };
        let key = self.fields.keys.get(config.api_key_name()).map_or("", |key| key.trim()).to_owned();
        if key.is_empty() {
            "Enter an API key first.".clone_into(&mut self.test_status);
            return false;
        }
        let label = config.label();
        self.test_status = format!("Testing {label} with a short sample…");
        let started = Instant::now();
        let (results, inbox) = mpsc::channel();
        let create_client = self.create_client.clone();
        let timeout_s = config.timeout_s;
        thread::spawn(move || {
            let client = create_client(&config, &key);
            let result = match client.polish(TEST_SAMPLE, "") {
                Ok(_) => format!("{label} replied in {:.1} s.", started.elapsed().as_secs_f64()),
                Err(e) => format!("{label}: {e}"),
            };
            let _ = results.send(result);
        });
        self.test = Some(ProviderTest { results: inbox, started, label, timeout_s });
        true
    }

    /// See whether the provider test ended. True while it still runs.
    pub fn poll_provider_test(&mut self) -> bool {
        let Some(test) = &self.test else {
            return false;
        };
        let result = match test.results.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) if test.started.elapsed().as_secs_f64() < test.timeout_s => return true,
            Err(TryRecvError::Empty) => format!("{} did not reply within {} s.", test.label, short_float(test.timeout_s)),
            Err(TryRecvError::Disconnected) => format!("Could not connect to {}. Check the key and endpoint.", test.label),
        };
        self.test_status = result;
        self.test = None;
        false
    }
}

fn is_model_folder(folder: &Path) -> bool {
    folder.join("config.json").is_file()
}
