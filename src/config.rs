use std::env;
use std::fmt;
use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::app_rules::AppRule;
use crate::dictionary::{PersonalDictionary, Replacement};
use crate::hotkeys;
use crate::paths;
use crate::text::{casefold, split_lines, strip_bom};

pub const STYLES: [&str; 2] = ["fluid", "bars"];
pub const DEFAULT_STYLE: &str = "fluid";
pub const OVERLAY_CHOICES: [&str; 3] = ["fluid", "bars", "off"];

const ZAI_MODEL: &str = "glm-5.3";
const ZAI_ENDPOINT: &str = "https://api.z.ai/api/coding/paas/v4/chat/completions";
const CLAUDE_MODEL: &str = "claude-haiku-5-5";
const CLAUDE_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

fn error<T>(message: impl Into<String>) -> Result<T, ConfigError> {
    Err(ConfigError(message.into()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolishProvider {
    Zai,
    Anthropic,
}

impl PolishProvider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Zai => "zai",
            Self::Anthropic => "anthropic",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PostprocessConfig {
    pub enabled: bool,
    pub provider: PolishProvider,
    pub model: String,
    pub endpoint: String,
    pub reasoning_effort: String,
    pub thinking: bool,
    pub effort: String,
    pub timeout_s: f64,
    pub extra_instructions: String,
}

impl Default for PostprocessConfig {
    fn default() -> Self {
        Self::for_provider(PolishProvider::Zai)
    }
}

impl PostprocessConfig {
    pub fn for_provider(provider: PolishProvider) -> Self {
        let (model, endpoint) = match provider {
            PolishProvider::Zai => (ZAI_MODEL, ZAI_ENDPOINT),
            PolishProvider::Anthropic => (CLAUDE_MODEL, CLAUDE_ENDPOINT),
        };
        Self {
            enabled: true,
            provider,
            model: model.to_owned(),
            endpoint: endpoint.to_owned(),
            reasoning_effort: "low".to_owned(),
            thinking: false,
            effort: "low".to_owned(),
            timeout_s: 8.0,
            extra_instructions: String::new(),
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.provider == PolishProvider::Anthropic && !self.thinking && matches!(self.effort.as_str(), "xhigh" | "max") {
            return error("postprocess.effort above 'high' requires thinking = true for Anthropic");
        }
        Ok(())
    }

    pub fn label(&self) -> &'static str {
        match self.provider {
            PolishProvider::Anthropic => "Claude",
            PolishProvider::Zai => "GLM",
        }
    }

    pub fn api_key_name(&self) -> &'static str {
        match self.provider {
            PolishProvider::Anthropic => "ANTHROPIC_API_KEY",
            PolishProvider::Zai => "ZAI_API_KEY",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub enabled: bool,
    pub port: u16,
    pub bind: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { enabled: false, port: 8765, bind: "0.0.0.0".to_owned() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Cuda,
    Cpu,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cuda => "cuda",
            Self::Cpu => "cpu",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub hotkey: String,
    pub tap_threshold_ms: u32,
    pub provider: Engine,
    pub sounds: bool,
    pub append_space: bool,
    /// One of OVERLAY_CHOICES: a style name, or "off".
    pub overlay: String,
    pub model_dir: PathBuf,
    pub postprocess: PostprocessConfig,
    pub server: ServerConfig,
    pub app_rules: Vec<AppRule>,
    pub dictionary: PersonalDictionary,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "ctrl_r".to_owned(),
            tap_threshold_ms: 300,
            provider: Engine::Cuda,
            sounds: true,
            append_space: true,
            overlay: DEFAULT_STYLE.to_owned(),
            model_dir: paths::root().join("models").join("parakeet-ultra"),
            postprocess: PostprocessConfig::default(),
            server: ServerConfig::default(),
            app_rules: Vec::new(),
            dictionary: PersonalDictionary::default(),
        }
    }
}

impl Config {
    pub fn hotkey_vk(&self) -> u32 {
        hotkeys::vk_of(&self.hotkey).expect("parse_config accepts only known keys")
    }

    pub fn hotkey_label(&self) -> String {
        hotkeys::label_of(&self.hotkey)
    }
}

pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let Ok(bytes) = fs::read(path) else {
        return Ok(Config::default());
    };
    let text = String::from_utf8(bytes).map_err(|e| ConfigError(format!("{}: {e}", path.display())))?;
    let raw: Table = toml::from_str(strip_bom(&text)).map_err(|e| ConfigError(format!("{}: {}", path.display(), e.message())))?;
    parse_config(&raw)
}

/// A value the way a person reads it in an error message.
fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => format!("'{text}'"),
        Value::Boolean(flag) => if *flag { "True" } else { "False" }.to_owned(),
        Value::Integer(number) => number.to_string(),
        Value::Float(number) => number.to_string(),
        other => other.to_string(),
    }
}

fn unknown_keys(table: &Table, known: &[&str]) -> Vec<String> {
    let mut unknown: Vec<String> = table.keys().filter(|key| !known.contains(&key.as_str())).cloned().collect();
    unknown.sort();
    unknown
}

/// Validate settings from either TOML or the settings window before applying them.
pub fn parse_config(raw: &Table) -> Result<Config, ConfigError> {
    const KNOWN: [&str; 11] = [
        "hotkey", "tap_threshold_ms", "provider", "sounds", "append_space", "overlay", "model_dir", "postprocess", "server", "app_rules",
        "dictionary",
    ];
    let unknown = unknown_keys(raw, &KNOWN);
    if !unknown.is_empty() {
        return error(format!("unknown setting(s) {unknown:?}"));
    }
    let mut config = Config::default();
    for (name, value) in raw {
        match name.as_str() {
            "hotkey" => config.hotkey = parse_hotkey(value)?,
            "tap_threshold_ms" => match value.as_integer() {
                Some(ms) if (1..=2000).contains(&ms) => config.tap_threshold_ms = ms as u32,
                _ => return error(format!("tap_threshold_ms must be an integer in 1..2000; got {}", shown(value))),
            },
            "provider" => match value.as_str() {
                Some("cuda") => config.provider = Engine::Cuda,
                Some("cpu") => config.provider = Engine::Cpu,
                _ => return error(format!("provider must be 'cuda' or 'cpu'; got {}", shown(value))),
            },
            "overlay" => match value.as_str() {
                Some(style) if OVERLAY_CHOICES.contains(&style) => config.overlay = style.to_owned(),
                _ => return error(format!("overlay must be one of 'fluid', 'bars', 'off'; got {}", shown(value))),
            },
            "sounds" | "append_space" => {
                let Some(flag) = value.as_bool() else {
                    return error(format!("{name} must be true or false; got {}", shown(value)));
                };
                if name == "sounds" {
                    config.sounds = flag;
                } else {
                    config.append_space = flag;
                }
            }
            "model_dir" => match value.as_str() {
                Some(path) => config.model_dir = paths::root().join(path),
                None => return error(format!("model_dir must be a path string; got {}", shown(value))),
            },
            "postprocess" => config.postprocess = parse_postprocess(value)?,
            "server" => config.server = parse_server(value)?,
            "app_rules" => config.app_rules = parse_app_rules(value)?,
            "dictionary" => config.dictionary = parse_dictionary(value)?,
            _ => unreachable!("checked against KNOWN"),
        }
    }
    Ok(config)
}

fn parse_hotkey(value: &Value) -> Result<String, ConfigError> {
    let Some(name) = value.as_str().filter(|name| hotkeys::vk_of(name).is_some()) else {
        return error(format!("hotkey must be a pynput Key name such as 'ctrl_r', 'f13', 'pause'; got {}", shown(value)));
    };
    if name == "esc" {
        return error("hotkey cannot be 'esc'; Esc cancels a recording");
    }
    // The keyboard hook reports sided modifiers only, so a generic one would never match.
    if matches!(name, "ctrl" | "shift" | "alt") {
        return error(format!("hotkey must name a side, like '{name}_r'; got '{name}'"));
    }
    Ok(name.to_owned())
}

fn parse_postprocess(value: &Value) -> Result<PostprocessConfig, ConfigError> {
    const KNOWN: [&str; 9] =
        ["enabled", "provider", "model", "endpoint", "reasoning_effort", "thinking", "effort", "timeout_s", "extra_instructions"];
    let Some(table) = value.as_table() else {
        return error(format!("postprocess must be a table, [postprocess]; got {}", shown(value)));
    };
    let unknown = unknown_keys(table, &KNOWN);
    if !unknown.is_empty() {
        return error(format!("unknown postprocess setting(s) {unknown:?}"));
    }
    // The provider first: it chooses the default model and endpoint.
    let provider = match table.get("provider") {
        None => PolishProvider::Zai,
        Some(item) => match item.as_str() {
            Some("zai") => PolishProvider::Zai,
            Some("anthropic") => PolishProvider::Anthropic,
            _ => return error(format!("postprocess.provider must be 'zai' or 'anthropic'; got {}", shown(item))),
        },
    };
    let mut config = PostprocessConfig::for_provider(provider);
    for (key, item) in table {
        match key.as_str() {
            "provider" => {}
            "enabled" | "thinking" => {
                let Some(flag) = item.as_bool() else {
                    return error(format!("postprocess.{key} must be true or false; got {}", shown(item)));
                };
                if key == "enabled" {
                    config.enabled = flag;
                } else {
                    config.thinking = flag;
                }
            }
            "model" | "endpoint" => {
                let Some(text) = item.as_str().filter(|text| !text.trim().is_empty()) else {
                    return error(format!("postprocess.{key} must be a non-empty string; got {}", shown(item)));
                };
                if key == "endpoint" && !(text.starts_with("https://") || text.starts_with("http://")) {
                    return error(format!("postprocess.endpoint must be an http(s) URL; got {}", shown(item)));
                }
                if key == "model" {
                    config.model = text.to_owned();
                } else {
                    config.endpoint = text.to_owned();
                }
            }
            "reasoning_effort" => match item.as_str() {
                Some(effort @ ("low" | "high" | "max")) => config.reasoning_effort = effort.to_owned(),
                _ => return error(format!("postprocess.reasoning_effort must be 'low', 'high', or 'max'; got {}", shown(item))),
            },
            "effort" => match item.as_str() {
                Some(effort @ ("low" | "medium" | "high" | "xhigh" | "max")) => config.effort = effort.to_owned(),
                _ => return error(format!("postprocess.effort must be 'low', 'medium', 'high', 'xhigh', or 'max'; got {}", shown(item))),
            },
            "timeout_s" => {
                let seconds = match item {
                    Value::Integer(number) => *number as f64,
                    Value::Float(number) => *number,
                    _ => f64::NAN,
                };
                if !(seconds > 0.0 && seconds <= 60.0) {
                    return error(format!("postprocess.timeout_s must be a number of seconds in (0, 60]; got {}", shown(item)));
                }
                config.timeout_s = seconds;
            }
            "extra_instructions" => match item.as_str() {
                Some(text) => config.extra_instructions = text.to_owned(),
                None => return error(format!("postprocess.extra_instructions must be a string; got {}", shown(item))),
            },
            _ => unreachable!("checked against KNOWN"),
        }
    }
    config.validate()?;
    Ok(config)
}

fn parse_server(value: &Value) -> Result<ServerConfig, ConfigError> {
    let Some(table) = value.as_table() else {
        return error(format!("server must be a table, [server]; got {}", shown(value)));
    };
    let unknown = unknown_keys(table, &["enabled", "port", "bind"]);
    if !unknown.is_empty() {
        return error(format!("unknown server setting(s) {unknown:?}"));
    }
    let mut config = ServerConfig::default();
    for (key, item) in table {
        match key.as_str() {
            "enabled" => match item.as_bool() {
                Some(flag) => config.enabled = flag,
                None => return error(format!("server.enabled must be true or false; got {}", shown(item))),
            },
            "port" => match item.as_integer() {
                Some(port) if (1..=65535).contains(&port) => config.port = port as u16,
                _ => return error(format!("server.port must be an integer in 1..65535; got {}", shown(item))),
            },
            _ => match item.as_str().filter(|text| text.parse::<Ipv4Addr>().is_ok()) {
                Some(address) => config.bind = address.to_owned(),
                None => return error(format!("server.bind must be an IPv4 address such as '0.0.0.0'; got {}", shown(item))),
            },
        }
    }
    Ok(config)
}

fn parse_app_rules(value: &Value) -> Result<Vec<AppRule>, ConfigError> {
    let Some(rules) = value.as_array() else {
        return error("app_rules must be an array of tables, [[app_rules]]");
    };
    rules.iter().enumerate().map(|(index, rule)| parse_app_rule(rule, index + 1)).collect()
}

fn parse_app_rule(value: &Value, index: usize) -> Result<AppRule, ConfigError> {
    let label = format!("app_rules[{index}]");
    let Some(table) = value.as_table() else {
        return error(format!("{label} must be a table"));
    };
    let unknown = unknown_keys(table, &["name", "instructions", "apps", "enabled"]);
    if !unknown.is_empty() {
        return error(format!("unknown {label} setting(s) {unknown:?}"));
    }
    let Some(name) = table.get("name").and_then(Value::as_str).filter(|name| !name.trim().is_empty()) else {
        return error(format!("{label}.name must be a non-empty string"));
    };
    let Some(instructions) = table.get("instructions").and_then(Value::as_str) else {
        return error(format!("{label}.instructions must be a string"));
    };
    let enabled = match table.get("enabled") {
        None => true,
        Some(item) => match item.as_bool() {
            Some(flag) => flag,
            None => return error(format!("{label}.enabled must be true or false")),
        },
    };
    let apps: Option<Vec<String>> = match table.get("apps") {
        None => Some(Vec::new()),
        Some(item) => item.as_array().and_then(|apps| {
            apps.iter().map(|app| app.as_str().filter(|app| is_executable_name(app)).map(str::to_owned)).collect()
        }),
    };
    let Some(apps) = apps else {
        return error(format!("{label}.apps must be executable names such as ['Slack.exe'], without paths or wildcards"));
    };
    Ok(AppRule { name: name.to_owned(), instructions: instructions.to_owned(), apps, enabled })
}

pub fn is_executable_name(app: &str) -> bool {
    app == app.trim()
        && app.chars().count() > 4
        && casefold(app).ends_with(".exe")
        && !app.chars().any(|c| "\\/:*?\"<>|\0".contains(c))
}

fn parse_dictionary(value: &Value) -> Result<PersonalDictionary, ConfigError> {
    let Some(table) = value.as_table().filter(|table| unknown_keys(table, &["words", "replacements"]).is_empty()) else {
        return error("dictionary must be a [dictionary] table with words and replacements");
    };
    let empty = Vec::new();
    let words = table.get("words").map_or(Some(&empty), Value::as_array);
    let replacements = table.get("replacements").map_or(Some(&empty), Value::as_array);
    let (Some(words), Some(replacements)) = (words, replacements) else {
        return error("dictionary.words and dictionary.replacements must be arrays");
    };
    let mut entries = Vec::with_capacity(replacements.len());
    for entry in replacements {
        let pair = entry.as_table().filter(|table| table.len() == 2).and_then(|table| {
            Some(Replacement::new(table.get("source")?.as_str()?, table.get("target")?.as_str()?))
        });
        match pair {
            Some(pair) => entries.push(pair),
            None => return error("dictionary replacements must have source and target strings"),
        }
    }
    let Some(words) = words.iter().map(|word| word.as_str().map(str::to_owned)).collect::<Option<Vec<_>>>() else {
        return error("Dictionary entries must be non-empty single lines without surrounding spaces.");
    };
    PersonalDictionary::new(words, entries).map_err(ConfigError)
}

pub fn load_api_key(env_path: &Path, name: &str) -> Option<String> {
    if let Ok(value) = env::var(name) {
        let key = value.trim();
        if !key.is_empty() {
            return Some(key.to_owned());
        }
    }
    let bytes = fs::read(env_path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    for line in split_lines(strip_bom(&text)) {
        let (entry, value) = line.split_once('=').unwrap_or((line, ""));
        if entry.trim() == name {
            let value = value.trim().trim_matches(['"', '\'']);
            return (!value.is_empty()).then(|| value.to_owned());
        }
    }
    None
}
