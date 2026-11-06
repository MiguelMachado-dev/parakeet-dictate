//! Where the app keeps its files. Everything lives in one folder: config.toml, .env, tray_choices.json,
//! server_token.txt, dictate.log, models\ and runtime\.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// `PARAKEET_DICTATE_ROOT` when set. Otherwise the nearest folder above the executable with config.example.toml,
/// which is the project root both for target\release and for an installed copy, or the executable's own folder.
pub fn root() -> &'static Path {
    ROOT.get_or_init(|| {
        if let Some(root) = env::var_os("PARAKEET_DICTATE_ROOT").filter(|value| !value.is_empty()) {
            return PathBuf::from(root);
        }
        let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
        let folder = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
        folder.ancestors().find(|dir| dir.join("config.example.toml").is_file()).unwrap_or(&folder).to_path_buf()
    })
}

pub fn config() -> PathBuf {
    root().join("config.toml")
}

pub fn log() -> PathBuf {
    root().join("dictate.log")
}

pub fn env_file() -> PathBuf {
    root().join(".env")
}

pub fn tray_choices() -> PathBuf {
    root().join("tray_choices.json")
}

pub fn server_token() -> PathBuf {
    root().join("server_token.txt")
}

pub fn runtime() -> PathBuf {
    root().join("runtime")
}

/// `name` with `.tmp` added beside it, the file an atomic write goes through.
pub fn temporary_beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}
