use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use log::{error, warn};
use serde_json::{Map, Value};

use crate::config::{Config, OVERLAY_CHOICES};
use crate::paths::temporary_beside;

fn is_valid(key: &str, value: &Value) -> bool {
    match key {
        "postprocess" | "server" => value.is_boolean(),
        "overlay" => value.as_str().is_some_and(|style| OVERLAY_CHOICES.contains(&style)),
        _ => false,
    }
}

/// Invalid saved choices fall back to config.toml, both in the app and in Settings.
pub fn parse_choices(data: Option<&[u8]>, name: &str) -> Map<String, Value> {
    let Some(data) = data else {
        return Map::new();
    };
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let Ok(Value::Object(saved)) = serde_json::from_slice::<Value>(data) else {
        warn!("ignoring {name}: expected a JSON object");
        return Map::new();
    };
    let mut chosen = Map::new();
    for (key, value) in saved {
        if is_valid(&key, &value) {
            chosen.insert(key, value);
        } else {
            warn!("ignoring {key:?} in {name}: {value}");
        }
    }
    chosen
}

fn read_choices(path: &Path) -> io::Result<Map<String, Value>> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match fs::read(path) {
        Ok(data) => Ok(parse_choices(Some(&data), &name)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(e),
    }
}

/// Tray choices override config.toml and survive restarts. Unchosen keys use the config.
pub struct TrayChoices {
    path: PathBuf,
    config_postprocess: bool,
    config_overlay: String,
    config_server: bool,
    chosen: Mutex<Map<String, Value>>,
}

impl TrayChoices {
    pub fn new(config: &Config, path: PathBuf) -> Self {
        let chosen = read_choices(&path).unwrap_or_else(|e| {
            warn!("could not read {}; using config.toml: {e}", path.display());
            Map::new()
        });
        Self {
            path,
            config_postprocess: config.postprocess.enabled,
            config_overlay: config.overlay.clone(),
            config_server: config.server.enabled,
            chosen: Mutex::new(chosen),
        }
    }

    pub fn postprocess(&self) -> bool {
        self.chosen.lock().unwrap().get("postprocess").and_then(Value::as_bool).unwrap_or(self.config_postprocess)
    }

    /// A style name or "off".
    pub fn overlay(&self) -> String {
        let chosen = self.chosen.lock().unwrap();
        chosen.get("overlay").and_then(Value::as_str).map_or_else(|| self.config_overlay.clone(), str::to_owned)
    }

    pub fn server(&self) -> bool {
        self.chosen.lock().unwrap().get("server").and_then(Value::as_bool).unwrap_or(self.config_server)
    }

    pub fn choose(&self, key: &str, value: Value) {
        self.chosen.lock().unwrap().insert(key.to_owned(), value.clone());
        let temporary = temporary_beside(&self.path, ".tray.tmp");
        let saved = read_choices(&self.path).and_then(|mut saved| {
            // Settings can save while this process waits for an active dictation to finish. Only persist
            // this menu action; never replay this process's older choices.
            saved.insert(key.to_owned(), value);
            fs::write(&temporary, serde_json::to_string(&saved).expect("choices serialize"))?;
            fs::rename(&temporary, &self.path)
        });
        if let Err(e) = saved {
            error!("could not save the tray choices: {e}");
        }
        let _ = fs::remove_file(&temporary);
    }
}
