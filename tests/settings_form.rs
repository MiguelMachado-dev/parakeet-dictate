use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use parakeet_dictate::config::{PolishProvider, PostprocessConfig};
use parakeet_dictate::instance::create_event;
use parakeet_dictate::polish::{Polish, PolishError};
use parakeet_dictate::settings::apply_settings;
use parakeet_dictate::settings::form::Editor;
use parakeet_dictate::settings_store::{KEY_NAMES, SettingsStore};
use tempfile::{TempDir, tempdir};
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

static ENV: Mutex<()> = Mutex::new(());

const CLAUDE: &str = "Anthropic (Claude)";
const GLM: &str = "Z.ai (GLM)";

type Calls = Arc<Mutex<Vec<String>>>;

/// A provider that answers at once and notes what it was asked; it never reaches a network.
struct Client {
    calls: Calls,
}

impl Polish for Client {
    fn preconnect(&self) {}

    fn polish(&self, raw: &str, _instructions: &str) -> Result<String, PolishError> {
        self.calls.lock().unwrap().push(raw.to_owned());
        Ok(raw.to_owned())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.calls.lock().unwrap().push("closed".to_owned());
    }
}

/// The `window` fixture: an Editor over an empty folder, with neither key in the environment.
struct Fixture {
    editor: Editor,
    applied: Rc<Cell<u32>>,
    calls: Calls,
    root: PathBuf,
    saved: Vec<(&'static str, Option<OsString>)>,
    _dir: TempDir,
    _lock: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let lock = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = KEY_NAMES.iter().map(|name| (*name, env::var_os(name))).collect();
        for name in KEY_NAMES {
            // SAFETY: ENV serialises every test in this binary that reads or changes the key variables.
            unsafe { env::remove_var(name) };
        }
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let applied = Rc::new(Cell::new(0));
        let calls = Calls::default();
        let count = applied.clone();
        let on_apply = Box::new(move || {
            count.set(count.get() + 1);
            Ok("Applied in test.".to_owned())
        });
        let recorded = calls.clone();
        let create_client = Arc::new(move |_: &PostprocessConfig, _: &str| -> Arc<dyn Polish> { Arc::new(Client { calls: recorded.clone() }) });
        let editor = Editor::with_client(SettingsStore::new(root.clone()), on_apply, create_client).unwrap();
        Self { editor, applied, calls, root, saved, _dir: dir, _lock: lock }
    }

    fn files(&self) -> Vec<String> {
        fs::read_dir(&self.root).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect()
    }

    fn assert_nothing_written_or_applied(&self) {
        assert_eq!(self.files(), Vec::<String>::new());
        assert_eq!(self.applied.get(), 0);
    }

    fn set_key(&mut self, name: &str, value: &str) {
        self.editor.fields.keys.insert(name.to_owned(), value.to_owned());
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

fn path_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// The window's handler for the thinking checkbox.
fn set_thinking(editor: &mut Editor, thinking: bool) {
    editor.fields.thinking = thinking;
    editor.limit_effort();
    editor.changed();
}

// Dropped from the Python test: the notebook's tab titles and the masked key entry are window.rs controls.
#[test]
fn test_opening_settings_does_not_write_files_or_apply_changes() {
    let fixture = Fixture::new();
    assert!(!fixture.editor.dirty);
    fixture.assert_nothing_written_or_applied();
}

#[test]
fn test_provider_switching_keeps_keys_separate_and_limits_thinking_off_effort() {
    let mut fixture = Fixture::new();
    fixture.set_key("ZAI_API_KEY", "zai-test");
    fixture.editor.change_provider(CLAUDE);
    fixture.set_key("ANTHROPIC_API_KEY", "claude-test");
    assert_eq!(fixture.editor.fields.polish_model, "claude-haiku-5-5");
    assert_eq!(fixture.editor.fields.endpoint, "https://api.anthropic.com/v1/messages");
    set_thinking(&mut fixture.editor, true);
    fixture.editor.fields.effort = "Max".to_owned();
    set_thinking(&mut fixture.editor, false);
    assert_eq!(fixture.editor.fields.effort, "High");
    assert_eq!(fixture.editor.allowed_efforts(), ["Low", "Medium", "High"]);
    fixture.editor.change_provider(GLM);
    assert_eq!(fixture.editor.fields.polish_model, "glm-5.3");
    fixture.editor.change_provider(CLAUDE);
    assert!(fixture.editor.save());
    let saved = fixture.editor.store.load().unwrap();
    let keys = BTreeMap::from([
        ("ZAI_API_KEY".to_owned(), "zai-test".to_owned()),
        ("ANTHROPIC_API_KEY".to_owned(), "claude-test".to_owned()),
    ]);
    assert_eq!(saved.keys, keys);
    assert_eq!(saved.config.postprocess.provider, PolishProvider::Anthropic);
    assert_eq!(saved.config.postprocess.effort, "high");
    assert_eq!(fixture.applied.get(), 1);
}

#[test]
fn test_rules_can_be_added_edited_disabled_reordered_removed_and_saved() {
    let mut fixture = Fixture::new();
    let editor = &mut fixture.editor;
    editor.add_rule("English", "Translate to English.");
    assert!(editor.add_app("Slack.exe"));
    assert!(editor.add_app("SLACK.EXE"));
    assert_eq!(editor.fields.rules[0].apps, ["Slack.exe"]);
    editor.add_rule("Casual", "Keep it casual.");
    assert!(editor.add_app("Discord.exe"));
    editor.selected_rule().unwrap().enabled = false;
    editor.move_rule(-1);
    let names: Vec<&str> = editor.fields.rules.iter().map(|rule| rule.name.as_str()).collect();
    assert_eq!(names, ["Casual", "English"]);
    assert!(editor.save());
    let enabled: Vec<bool> = editor.store.load().unwrap().config.app_rules.iter().map(|rule| rule.enabled).collect();
    assert_eq!(enabled, [false, true]);
    editor.remove_rule();
    assert!(editor.save());
    let saved = editor.store.load().unwrap().config.app_rules;
    let names: Vec<&str> = saved.iter().map(|rule| rule.name.as_str()).collect();
    assert_eq!(names, ["English"]);
    editor.reload();
    assert_eq!(editor.fields.rules, editor.store.load().unwrap().config.app_rules);
}

#[test]
fn test_dictionary_example_preview_validation_and_save() {
    let mut fixture = Fixture::new();
    let editor = &mut fixture.editor;
    editor.fields.sample = "PY".to_owned();
    assert!(!editor.dirty);
    editor.dictionary_example();
    assert_eq!(editor.dictionary_preview(), "PI");
    assert!(editor.save());
    let saved = editor.store.load().unwrap().config.dictionary;
    assert_eq!(saved.words(), ["PI", "harness"]);
    assert_eq!(saved.apply("O PY é o HARNESS."), "O PI é o harness.");
    editor.add_row(0);
    editor.fields.replacements[1].0 = "harnes".to_owned();
    assert!(!editor.save());
    assert!(editor.status.contains("row 2"), "{}", editor.status);
    assert_eq!(editor.store.load().unwrap().config.dictionary, saved);
    editor.fields.replacements[1].1 = "harness".to_owned();
    assert!(editor.save());
    assert_eq!(editor.store.load().unwrap().config.dictionary.apply("harnes"), "harness");
    editor.remove_row(1);
    assert!(editor.save());
    editor.dirty = false;
    editor.reload();
    assert_eq!(editor.draft().unwrap().dictionary, saved);
    let last = editor.fields.replacements.len() - 1;
    editor.add_row(last);
    assert_eq!(editor.draft().unwrap().dictionary, saved); // A completely empty draft row is not persisted.
    editor.remove_row(last + 1);
}

#[test]
fn test_invalid_values_or_external_edits_do_not_apply() {
    let mut fixture = Fixture::new();
    fixture.editor.fields.server_port = "not a port".to_owned();
    assert!(!fixture.editor.save());
    assert!(fixture.editor.status.contains("server.port"), "{}", fixture.editor.status);
    fixture.assert_nothing_written_or_applied();
    fixture.editor.fields.server_port = "8765".to_owned();
    let choices = fixture.root.join("tray_choices.json");
    fs::write(&choices, r#"{"overlay":"bars"}"#).unwrap();
    assert!(!fixture.editor.save());
    assert!(fixture.editor.status.contains("Reload"), "{}", fixture.editor.status);
    assert_eq!(fixture.applied.get(), 0);
    assert_eq!(path_text(&choices), r#"{"overlay":"bars"}"#);
}

// The yes/no dialogs live in window.rs, which only calls `Editor::reload` or closes when `dirty` is false or the
// user says yes. Here: the state those checks read, and that discarding leaves the disk alone.
#[test]
fn test_closing_or_declining_reload_preserves_unsaved_disk_state() {
    let mut fixture = Fixture::new();
    fixture.editor.fields.sounds = false;
    fixture.editor.changed();
    assert!(fixture.editor.dirty);
    assert!(!fixture.editor.fields.sounds);
    fixture.assert_nothing_written_or_applied();
}

#[test]
fn test_key_from_environment_is_masked_and_read_only() {
    let mut fixture = Fixture::new();
    assert!(!fixture.editor.key_is_managed());
    // SAFETY: ENV serialises these tests, and the fixture restores the variable.
    unsafe { env::set_var("ZAI_API_KEY", "environment-test") };
    fixture.editor.reload();
    assert!(fixture.editor.key_is_managed());
    assert!(fixture.editor.key_hint().contains("environment variable"), "{}", fixture.editor.key_hint());
}

#[test]
fn test_test_provider_uses_the_draft_without_saving_and_keeps_the_window_responsive() {
    let mut fixture = Fixture::new();
    fixture.editor.change_provider(CLAUDE);
    fixture.set_key("ANTHROPIC_API_KEY", "test-key");
    assert!(fixture.editor.test_provider());
    assert!(fixture.editor.testing());
    let deadline = Instant::now() + Duration::from_secs(3);
    while fixture.editor.poll_provider_test() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(fixture.editor.test_status.contains("Claude replied"), "{}", fixture.editor.test_status);
    // The worker thread drops its client just after it reports the result.
    while fixture.calls.lock().unwrap().len() < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(*fixture.calls.lock().unwrap(), ["um hello this is a dictation test", "closed"]);
    fixture.assert_nothing_written_or_applied();
}

#[test]
fn test_loading_a_saved_provider_and_rules_back_into_widgets() {
    let mut fixture = Fixture::new();
    let editor = &mut fixture.editor;
    let snapshot = editor.store.load().unwrap();
    let mut config = snapshot.config.clone();
    config.overlay = "bars".to_owned();
    config.postprocess =
        PostprocessConfig { thinking: true, effort: "max".to_owned(), ..PostprocessConfig::for_provider(PolishProvider::Anthropic) };
    editor.store.save(&snapshot, &config, &snapshot.keys).unwrap();
    editor.reload();
    assert_eq!(editor.fields.overlay, "Bars");
    assert_eq!(editor.fields.polish_provider, CLAUDE);
    assert!(editor.fields.thinking);
    assert_eq!(editor.fields.effort, "Max");
}

/// An event name of its own, unrelated to the app's, so nothing reaches a running app.
fn reload_event_name() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("Local\\settings-form-test-{}-{nanos}-{}", process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

#[test]
fn test_saving_while_the_app_runs_asks_it_to_reload() {
    let name = reload_event_name();
    let event = create_event(&name, true).unwrap();
    let launches = RefCell::new(Vec::<Vec<String>>::new());
    let status = apply_settings(&name, |arguments| {
        launches.borrow_mut().push(arguments.iter().map(|argument| (*argument).to_owned()).collect());
        Ok(())
    })
    .unwrap();
    assert!(status.contains("reload after the current dictation"), "{status}");
    assert_eq!(unsafe { WaitForSingleObject(event.0, 0) }, WAIT_OBJECT_0);
    assert!(launches.borrow().is_empty());
}

#[test]
fn test_saving_while_the_app_is_closed_starts_it() {
    let launches = RefCell::new(Vec::<Vec<String>>::new());
    let status = apply_settings(&reload_event_name(), |arguments| {
        launches.borrow_mut().push(arguments.iter().map(|argument| (*argument).to_owned()).collect());
        Ok(())
    })
    .unwrap();
    assert!(status.contains("Starting Parakeet Dictate"), "{status}");
    assert_eq!(*launches.borrow(), [["--notify-ready"]]);
}
