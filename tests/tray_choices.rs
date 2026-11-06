use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Once;

use log::{Level, LevelFilter, Log, Metadata, Record};
use parakeet_dictate::config::{Config, PostprocessConfig};
use parakeet_dictate::tray_choices::TrayChoices;
use serde_json::{Value, json};
use tempfile::tempdir;
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

fn config(postprocess: bool, overlay: &str) -> Config {
    Config {
        overlay: overlay.to_owned(),
        postprocess: PostprocessConfig { enabled: postprocess, ..PostprocessConfig::default() },
        ..Config::default()
    }
}

fn saved(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

thread_local! {
    static WARNINGS: Cell<usize> = const { Cell::new(0) };
}

/// Counts warnings on the logging thread, so tests running beside each other do not mix.
struct WarningCounter;

impl Log for WarningCounter {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record) {
        if record.level() == Level::Warn {
            WARNINGS.with(|count| count.set(count.get() + 1));
        }
    }

    fn flush(&self) {}
}

fn capture_warnings() {
    static LOGGER: WarningCounter = WarningCounter;
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&LOGGER).unwrap();
        log::set_max_level(LevelFilter::Warn);
    });
    WARNINGS.with(|count| count.set(0));
}

fn warnings() -> usize {
    WARNINGS.with(Cell::get)
}

#[test]
fn test_config_values_apply_until_the_tray_chooses() {
    for (postprocess, overlay) in [(true, "fluid"), (false, "off"), (true, "bars")] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tray_choices.json");
        let choices = TrayChoices::new(&config(postprocess, overlay), path.clone());
        assert_eq!((choices.postprocess(), choices.overlay().as_str()), (postprocess, overlay));
        assert!(!path.exists());
    }
}

#[test]
fn test_a_saved_choice_wins_over_the_config_after_a_restart() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tray_choices.json");
    TrayChoices::new(&config(true, "fluid"), path.clone()).choose("overlay", json!("off"));
    TrayChoices::new(&config(true, "fluid"), path.clone()).choose("postprocess", json!(false));
    let restarted = TrayChoices::new(&config(true, "bars"), path.clone());
    assert_eq!((restarted.postprocess(), restarted.overlay().as_str()), (false, "off"));
    assert_eq!(saved(&path), json!({"overlay": "off", "postprocess": false}));
}

#[test]
fn test_choosing_one_key_leaves_the_other_on_its_config_value() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tray_choices.json");
    TrayChoices::new(&config(true, "fluid"), path.clone()).choose("overlay", json!("bars"));
    assert!(!TrayChoices::new(&config(false, "fluid"), path.clone()).postprocess());
    assert_eq!(saved(&path), json!({"overlay": "bars"}));
}

#[test]
fn test_an_invalid_saved_value_falls_back_to_the_config_and_warns_once() {
    let contents = ["", "not json", "[]", r#"{"postprocess": "yes", "overlay": "orb"}"#, r#"{"postprocess": 1, "overlay": null}"#];
    for content in contents {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tray_choices.json");
        fs::write(&path, content).unwrap();
        capture_warnings();
        let choices = TrayChoices::new(&config(false, "bars"), path);
        let count = warnings();
        assert_eq!((choices.postprocess(), choices.overlay().as_str()), (false, "bars"));
        assert_eq!((choices.postprocess(), choices.overlay().as_str()), (false, "bars"));
        assert!((1..=2).contains(&count), "{content:?} warned {count} times");
        assert_eq!(warnings(), count, "{content:?} warned again on a later read");
    }
}

#[test]
fn test_one_invalid_key_keeps_the_valid_one() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tray_choices.json");
    fs::write(&path, r#"{"postprocess": false, "overlay": "orb"}"#).unwrap();
    let choices = TrayChoices::new(&config(true, "fluid"), path);
    assert_eq!((choices.postprocess(), choices.overlay().as_str()), (false, "fluid"));
}

#[test]
fn test_a_failed_save_leaves_the_previous_file_whole() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tray_choices.json");
    let choices = TrayChoices::new(&config(true, "fluid"), path.clone());
    choices.choose("overlay", json!("bars"));
    let before = fs::read_to_string(&path).unwrap();

    // Open without delete sharing: the file still reads, but the atomic rename over it fails for real.
    let held = OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&path).unwrap();
    choices.choose("postprocess", json!(false));
    drop(held);

    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    assert_eq!(TrayChoices::new(&config(true, "fluid"), path).overlay(), "bars");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "the temporary file stayed behind");
}
