//! The Settings window edits the same files as the app, including the choices saved by the tray. Saving
//! keeps config.toml's comments and any key the window does not know.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::json;
use toml::{Table, Value};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table as EditTable, value as edit_value};

use crate::config::{Config, ConfigError, load_api_key, parse_config};
use crate::paths::{self, temporary_beside};
use crate::text::{split_lines_keepends, strip_bom};
use crate::tray_choices::parse_choices;

pub const KEY_NAMES: [&str; 2] = ["ZAI_API_KEY", "ANTHROPIC_API_KEY"];
const FILE_NAMES: [&str; 3] = ["config.toml", ".env", "tray_choices.json"];

#[derive(Debug)]
pub enum SettingsError {
    Invalid(ConfigError),
    Conflict,
    Io(io::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(e) => e.fmt(f),
            Self::Conflict => f.write_str("Settings changed outside this window. Reload before saving so those changes are kept."),
            Self::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for SettingsError {}

impl From<ConfigError> for SettingsError {
    fn from(e: ConfigError) -> Self {
        Self::Invalid(e)
    }
}

impl From<io::Error> for SettingsError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Files = BTreeMap<&'static str, Option<Vec<u8>>>;

#[derive(Clone)]
pub struct SettingsSnapshot {
    pub config: Config,
    pub keys: BTreeMap<String, String>,
    pub environment_keys: BTreeSet<String>,
    pub files: Files,
}

impl fmt::Debug for SettingsSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SettingsSnapshot").field("environment_keys", &self.environment_keys).finish_non_exhaustive()
    }
}

/// Settings as the table config.toml would hold, the form `parse_config` validates.
pub fn config_values(config: &Config) -> Table {
    let mut values = Table::new();
    let text = |value: &str| Value::String(value.to_owned());
    values.insert("hotkey".into(), text(&config.hotkey));
    values.insert("tap_threshold_ms".into(), Value::Integer(config.tap_threshold_ms.into()));
    values.insert("provider".into(), text(config.provider.name()));
    values.insert("sounds".into(), Value::Boolean(config.sounds));
    values.insert("append_space".into(), Value::Boolean(config.append_space));
    values.insert("overlay".into(), text(&config.overlay));
    let model_dir = config.model_dir.strip_prefix(paths::root()).unwrap_or(&config.model_dir);
    values.insert("model_dir".into(), text(&model_dir.to_string_lossy()));

    let post = &config.postprocess;
    let mut postprocess = Table::new();
    postprocess.insert("enabled".into(), Value::Boolean(post.enabled));
    postprocess.insert("provider".into(), text(post.provider.name()));
    postprocess.insert("model".into(), text(&post.model));
    postprocess.insert("endpoint".into(), text(&post.endpoint));
    postprocess.insert("reasoning_effort".into(), text(&post.reasoning_effort));
    postprocess.insert("thinking".into(), Value::Boolean(post.thinking));
    postprocess.insert("effort".into(), text(&post.effort));
    postprocess.insert("timeout_s".into(), Value::Float(post.timeout_s));
    postprocess.insert("extra_instructions".into(), text(&post.extra_instructions));
    values.insert("postprocess".into(), Value::Table(postprocess));

    let mut server = Table::new();
    server.insert("enabled".into(), Value::Boolean(config.server.enabled));
    server.insert("port".into(), Value::Integer(config.server.port.into()));
    server.insert("bind".into(), text(&config.server.bind));
    values.insert("server".into(), Value::Table(server));

    let rules = config.app_rules.iter().map(|rule| {
        let mut table = Table::new();
        table.insert("name".into(), text(&rule.name));
        table.insert("instructions".into(), text(&rule.instructions));
        table.insert("apps".into(), Value::Array(rule.apps.iter().map(|app| text(app)).collect()));
        table.insert("enabled".into(), Value::Boolean(rule.enabled));
        Value::Table(table)
    });
    values.insert("app_rules".into(), Value::Array(rules.collect()));

    let mut dictionary = Table::new();
    dictionary.insert("words".into(), Value::Array(config.dictionary.words().iter().map(|word| text(word)).collect()));
    let replacements = config.dictionary.replacements().iter().map(|entry| {
        let mut table = Table::new();
        table.insert("source".into(), text(&entry.source));
        table.insert("target".into(), text(&entry.target));
        Value::Table(table)
    });
    dictionary.insert("replacements".into(), Value::Array(replacements.collect()));
    values.insert("dictionary".into(), Value::Table(dictionary));
    values
}

type Writer = fn(&Path, &str, &[u8]) -> io::Result<()>;

pub struct SettingsStore {
    pub root: PathBuf,
    write: Writer,
}

impl Default for SettingsStore {
    fn default() -> Self {
        Self::new(paths::root().to_path_buf())
    }
}

impl SettingsStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root, write: write_file }
    }

    /// A store that writes through `write`, so a test can make one file fail.
    pub fn with_writer(root: PathBuf, write: Writer) -> Self {
        Self { root, write }
    }

    fn files(&self) -> io::Result<Files> {
        let mut files = Files::new();
        for name in FILE_NAMES {
            let content = match fs::read(self.root.join(name)) {
                Ok(data) => Some(data),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            files.insert(name, content);
        }
        Ok(files)
    }

    pub fn load(&self) -> Result<SettingsSnapshot, SettingsError> {
        let files = self.files()?;
        let unreadable = || ConfigError("Could not read config.toml. Check its syntax.".to_owned());
        let text = decode(files["config.toml"].as_deref()).map_err(|_| unreadable())?;
        let raw: Table = toml::from_str(&text).map_err(|_| unreadable())?;
        let config = parse_config(&raw).map_err(|_| unreadable())?;
        let mut values = config_values(&config);
        for (key, value) in parse_choices(files["tray_choices.json"].as_deref(), "tray_choices.json") {
            match (key.as_str(), &value) {
                ("overlay", serde_json::Value::String(style)) => {
                    values.insert(key, Value::String(style.clone()));
                }
                ("postprocess" | "server", serde_json::Value::Bool(enabled)) => {
                    values[key.as_str()].as_table_mut().expect("a table").insert("enabled".into(), Value::Boolean(*enabled));
                }
                _ => {}
            }
        }
        let config = parse_config(&values)?;
        let environment_keys =
            KEY_NAMES.iter().filter(|name| env::var(name).is_ok_and(|value| !value.trim().is_empty())).map(|name| (*name).to_owned()).collect();
        let env_path = self.root.join(".env");
        let keys = KEY_NAMES.iter().map(|name| ((*name).to_owned(), load_api_key(&env_path, name).unwrap_or_default())).collect();
        Ok(SettingsSnapshot { config, keys, environment_keys, files })
    }

    pub fn save(
        &self,
        snapshot: &SettingsSnapshot,
        config: &Config,
        keys: &BTreeMap<String, String>,
    ) -> Result<SettingsSnapshot, SettingsError> {
        let values = config_values(config);
        parse_config(&values)?;
        let current = self.files()?;
        if current != snapshot.files {
            return Err(SettingsError::Conflict);
        }
        let text = decode(current["config.toml"].as_deref()).map_err(ConfigError)?;
        let mut doc: DocumentMut = text.parse().map_err(|_| ConfigError("Could not read config.toml. Check its syntax.".to_owned()))?;
        for (name, value) in &values {
            match value {
                Value::Array(rules) if name == "app_rules" => {
                    let same = doc.get(name).and_then(item_value).is_some_and(|saved| same_value(&saved, value));
                    if !same && !(rules.is_empty() && doc.get(name).is_none()) {
                        doc[name] = rules_item(rules);
                    } else if rules.is_empty() && doc.get(name).is_none() {
                        doc[name] = Item::Value(toml_edit::Array::new().into());
                    }
                }
                Value::Table(table) => {
                    if doc.get(name).is_none() {
                        doc[name] = Item::Table(EditTable::new());
                    }
                    for (key, item) in table {
                        let same = doc[name].get(key).and_then(item_value).is_some_and(|saved| same_value(&saved, item));
                        if !same {
                            doc[name][key] = plain_item(item);
                        }
                    }
                }
                _ => {
                    let same = doc.get(name).and_then(item_value).is_some_and(|saved| same_value(&saved, value));
                    if !same {
                        doc[name] = plain_item(value);
                    }
                }
            }
        }

        let original = current[".env"].clone().unwrap_or_default();
        let original_env = strip_bom(&String::from_utf8_lossy(&original)).to_owned();
        let mut env_text = original_env.clone();
        for name in KEY_NAMES {
            let value = keys.get(name).map_or("", |value| value.trim());
            if snapshot.environment_keys.contains(name) {
                continue;
            }
            if value.contains(['\r', '\n', '\0', '"', '\'']) {
                return Err(ConfigError(format!("{name} must be a single key, without quotes or line breaks.")).into());
            }
            if value != snapshot.keys[name] {
                let kept: String = split_lines_keepends(&env_text)
                    .into_iter()
                    .filter(|line| line.split_once('=').map_or(*line, |(entry, _)| entry).trim() != name)
                    .collect();
                env_text = kept;
                if !env_text.is_empty() && !env_text.ends_with('\n') {
                    env_text.push('\n');
                }
                if !value.is_empty() {
                    env_text.push_str(&format!("{name}={value}\n"));
                }
            }
        }

        let mut choices = parse_choices(current["tray_choices.json"].as_deref(), "tray_choices.json");
        choices.insert("overlay".into(), json!(config.overlay));
        choices.insert("postprocess".into(), json!(config.postprocess.enabled));
        choices.insert("server".into(), json!(config.server.enabled));

        let env_bytes = if env_text == original_env { original } else { env_text.into_bytes() };
        let updated: [(&str, Vec<u8>); 3] = [
            ("config.toml", doc.to_string().into_bytes()),
            (".env", env_bytes),
            ("tray_choices.json", serde_json::to_vec(&choices).expect("choices serialize")),
        ];
        let mut written = Vec::new();
        for (name, data) in &updated {
            if current[name].as_deref() == Some(data.as_slice()) || (data.is_empty() && current[name].is_none()) {
                continue;
            }
            if let Err(e) = (self.write)(&self.root, name, data) {
                for &done in written.iter().rev() {
                    let previous: &Option<Vec<u8>> = &current[done];
                    let _ = match previous {
                        None => fs::remove_file(self.root.join(done)),
                        Some(data) => write_file(&self.root, done, data),
                    };
                }
                return Err(e.into());
            }
            written.push(*name);
        }
        self.load()
    }
}

fn decode(data: Option<&[u8]>) -> Result<String, String> {
    let text = std::str::from_utf8(data.unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(strip_bom(text).to_owned())
}

pub fn write_file(root: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    let path = root.join(name);
    let temporary = temporary_beside(&path, ".tmp");
    let written = fs::write(&temporary, data).and_then(|()| fs::rename(&temporary, &path));
    let _ = fs::remove_file(&temporary);
    written
}

fn rules_item(rules: &[Value]) -> Item {
    if rules.is_empty() {
        return Item::Value(toml_edit::Array::new().into());
    }
    let mut tables = ArrayOfTables::new();
    for rule in rules {
        let mut table = EditTable::new();
        for (key, item) in rule.as_table().expect("a rule is a table") {
            table[key] = plain_item(item);
        }
        tables.push(table);
    }
    Item::ArrayOfTables(tables)
}

fn plain_item(value: &Value) -> Item {
    match value {
        Value::String(text) => edit_value(text.as_str()),
        Value::Integer(number) => edit_value(*number),
        Value::Float(number) => edit_value(*number),
        Value::Boolean(flag) => edit_value(*flag),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(inline_value(item));
            }
            Item::Value(array.into())
        }
        other => edit_value(other.to_string()),
    }
}

fn inline_value(value: &Value) -> toml_edit::Value {
    match value {
        Value::String(text) => text.as_str().into(),
        Value::Integer(number) => (*number).into(),
        Value::Float(number) => (*number).into(),
        Value::Boolean(flag) => (*flag).into(),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(inline_value(item));
            }
            array.into()
        }
        Value::Table(table) => {
            let mut inline = InlineTable::new();
            for (key, item) in table {
                inline.insert(key, inline_value(item));
            }
            inline.into()
        }
        Value::Datetime(date) => date.to_string().into(),
    }
}

/// What an edited document holds at this item, in the plain form `config_values` produces.
fn item_value(item: &Item) -> Option<Value> {
    match item {
        Item::None => None,
        Item::Value(value) => Some(from_edit_value(value)),
        Item::Table(table) => {
            let mut plain = Table::new();
            for (key, entry) in table.iter() {
                plain.insert(key.to_owned(), item_value(entry)?);
            }
            Some(Value::Table(plain))
        }
        Item::ArrayOfTables(tables) => {
            let mut rows = Vec::new();
            for table in tables.iter() {
                let mut plain = Table::new();
                for (key, entry) in table.iter() {
                    plain.insert(key.to_owned(), item_value(entry)?);
                }
                rows.push(Value::Table(plain));
            }
            Some(Value::Array(rows))
        }
    }
}

fn from_edit_value(value: &toml_edit::Value) -> Value {
    match value {
        toml_edit::Value::String(text) => Value::String(text.value().clone()),
        toml_edit::Value::Integer(number) => Value::Integer(*number.value()),
        toml_edit::Value::Float(number) => Value::Float(*number.value()),
        toml_edit::Value::Boolean(flag) => Value::Boolean(*flag.value()),
        toml_edit::Value::Datetime(date) => Value::String(date.value().to_string()),
        toml_edit::Value::Array(items) => Value::Array(items.iter().map(from_edit_value).collect()),
        toml_edit::Value::InlineTable(table) => {
            Value::Table(table.iter().map(|(key, item)| (key.to_owned(), from_edit_value(item))).collect())
        }
    }
}

/// Numbers compare by value and rule tables ignore their `enabled = true` default, so an untouched file stays untouched.
fn same_value(saved: &Value, wanted: &Value) -> bool {
    match (saved, wanted) {
        (Value::Integer(a), Value::Float(b)) | (Value::Float(b), Value::Integer(a)) => (*a as f64) == *b,
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_value(x, y)),
        (Value::Table(a), Value::Table(b)) => {
            let enabled = |table: &Table| table.get("enabled").is_none_or(|flag| flag == &Value::Boolean(true));
            let shared = a.iter().all(|(key, x)| b.get(key).is_some_and(|y| same_value(x, y)) || (key == "enabled" && enabled(a) && enabled(b)));
            shared && b.iter().all(|(key, _)| a.contains_key(key) || key == "apps" || (key == "enabled" && enabled(b)))
        }
        _ => saved == wanted,
    }
}
