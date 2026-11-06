//! Port of tests/test_dictation.py.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, Once};
use std::thread;
use std::time::{Duration, Instant};

use parakeet_dictate::app_rules::AppRule;
use parakeet_dictate::config::{Config, PolishProvider, PostprocessConfig};
use parakeet_dictate::cues::PlayCue;
use parakeet_dictate::dictation::{Dictation, DictationParts};
use parakeet_dictate::dictionary::PersonalDictionary;
use parakeet_dictate::polish::{Headers, Polish, PolishError, Reply, Transport, TransportError, create_polisher};
use parakeet_dictate::recorder::{Audio, Record};
use parakeet_dictate::reducer::{Cue, Effect, Event, State, Transition};
use parakeet_dictate::transcriber::Transcribe;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(5);
const BRIEFLY: Duration = Duration::from_millis(50);

/// threading.Event.
#[derive(Default)]
struct Flag {
    set: Mutex<bool>,
    changed: Condvar,
}

impl Flag {
    fn set(&self) {
        *self.set.lock().unwrap() = true;
        self.changed.notify_all();
    }

    fn clear(&self) {
        *self.set.lock().unwrap() = false;
    }

    fn is_set(&self) -> bool {
        *self.set.lock().unwrap()
    }

    fn wait(&self, limit: Duration) -> bool {
        let set = self.set.lock().unwrap();
        *self.changed.wait_timeout_while(set, limit, |set| !*set).unwrap().0
    }
}

struct FakeRecorder {
    started: Arc<Flag>,
}

impl Record for FakeRecorder {
    fn start(&mut self) -> Result<(), String> {
        self.started.set();
        Ok(())
    }

    fn stop(&mut self) -> Result<Audio, String> {
        Ok(Audio { samples: vec![0.0; 16_000], rate: 16_000 })
    }

    fn discard(&mut self) {}

    fn device_name(&self) -> String {
        "fixture".to_owned()
    }
}

struct FakeTranscriber;

impl Transcribe for FakeTranscriber {
    fn transcribe(&self, _audio: &Audio) -> Result<String, String> {
        Ok("hi".to_owned())
    }
}

struct Silent;

impl PlayCue for Silent {
    fn play(&self, _cue: Cue) {}
}

fn state_name(state: &State) -> &'static str {
    match state {
        State::Idle { .. } => "Idle",
        State::Recording { .. } => "Recording",
        State::Transcribing { .. } => "Transcribing",
        State::Polishing { .. } => "Polishing",
        State::AwaitingRelease { .. } => "AwaitingRelease",
    }
}

#[test]
fn test_the_observer_sees_each_transition_with_its_effects_before_they_run() {
    let seen = Arc::new(Mutex::new(Vec::<Transition>::new()));
    let seen_at_paste = Arc::new(Mutex::new(Vec::<Transition>::new()));
    let pasted = Arc::new(Flag::default());

    let (observer, copy, done) = (seen.clone(), seen_at_paste.clone(), pasted.clone());
    let seen_by_paste = seen.clone();
    let dictation = Dictation::new(DictationParts {
        config: Config::default(),
        recorder: Box::new(FakeRecorder { started: Arc::default() }),
        transcriber: Arc::new(FakeTranscriber),
        paste: Box::new(move |_text| {
            copy.lock().unwrap().extend(seen_by_paste.lock().unwrap().iter().cloned());
            done.set();
            Ok(())
        }),
        cues: Arc::new(Silent),
        on_transition: Box::new(move |transition| observer.lock().unwrap().push(transition.clone())),
        polisher: None,
        polish_enabled: Arc::new(|| true),
        foreground_app: Box::new(|| None),
        hotkey_down: Box::new(|| false),
    });
    dictation.post(Event::HotkeyDown(0.0));
    dictation.post(Event::HotkeyUp(2.0));
    let was_pasted = pasted.wait(WAIT);
    dictation.stop();
    assert!(was_pasted);

    let seen = seen.lock().unwrap();
    let summary: Vec<_> = seen.iter().map(|t| (state_name(&t.previous), state_name(&t.state), t.effects.clone())).collect();
    assert_eq!(
        summary,
        [
            ("Idle", "Recording", vec![Effect::StartRecording, Effect::PlaySound(Cue::Start)]),
            ("Recording", "Transcribing", vec![Effect::StopAndTranscribe, Effect::PlaySound(Cue::Stop)]),
            ("Transcribing", "Idle", vec![Effect::Paste("hi".to_owned())]),
        ]
    );
    assert_eq!(*seen_at_paste.lock().unwrap(), *seen);
}

struct GatedTranscriber {
    entered: Flag,
    release: Flag,
}

impl GatedTranscriber {
    fn new() -> Self {
        let transcriber = Self { entered: Flag::default(), release: Flag::default() };
        transcriber.release.set();
        transcriber
    }
}

impl Transcribe for GatedTranscriber {
    fn transcribe(&self, _audio: &Audio) -> Result<String, String> {
        self.entered.set();
        assert!(self.release.wait(WAIT), "transcription was not released");
        Ok("hi".to_owned())
    }
}

type Request = (String, String);

struct FakePolisher {
    sent: Sender<Request>,
    requests: Mutex<Receiver<Request>>,
    preconnected: Flag,
}

impl Default for FakePolisher {
    fn default() -> Self {
        let (sent, requests) = mpsc::channel();
        Self { sent, requests: Mutex::new(requests), preconnected: Flag::default() }
    }
}

impl FakePolisher {
    fn next_request(&self) -> Request {
        self.requests.lock().unwrap().recv_timeout(WAIT).expect("a polish request")
    }

    fn no_requests(&self) -> bool {
        self.requests.lock().unwrap().try_recv().is_err()
    }
}

impl Polish for FakePolisher {
    fn preconnect(&self) {
        self.preconnected.set();
    }

    fn polish(&self, raw: &str, instructions: &str) -> Result<String, PolishError> {
        self.sent.send((raw.to_owned(), instructions.to_owned())).unwrap();
        Ok("polished".to_owned())
    }
}

fn rules() -> Vec<AppRule> {
    vec![AppRule::new("English", "Translate to English.", &["Slack.exe"]), AppRule::new("Formal", "Be formal.", &["notepad.exe"])]
}

struct Setup {
    rules: Vec<AppRule>,
    enabled: bool,
    with_key: bool,
    polisher: Option<Arc<dyn Polish>>,
    postprocess: PostprocessConfig,
    dictionary: PersonalDictionary,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            rules: rules(),
            enabled: false,
            with_key: true,
            polisher: None,
            postprocess: PostprocessConfig::default(),
            dictionary: PersonalDictionary::default(),
        }
    }
}

struct Harness {
    front: Arc<Mutex<Option<String>>>,
    started: Arc<Flag>,
    transcriber: Arc<GatedTranscriber>,
    /// The default polisher, None when a test supplies its own or there is no key.
    polisher: Option<Arc<FakePolisher>>,
    pasted: Receiver<String>,
    transitions: Receiver<Transition>,
    enabled: Arc<AtomicBool>,
    dictation: Dictation,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.transcriber.release.set();
    }
}

impl Harness {
    fn start(setup: Setup) -> Self {
        let fake = (setup.polisher.is_none() && setup.with_key).then(|| Arc::new(FakePolisher::default()));
        let client = setup.polisher.or_else(|| fake.clone().map(|fake| fake as Arc<dyn Polish>));
        let front = Arc::new(Mutex::new(Some("Slack.exe".to_owned())));
        let started = Arc::new(Flag::default());
        let transcriber = Arc::new(GatedTranscriber::new());
        let enabled = Arc::new(AtomicBool::new(setup.enabled));
        let (paste, pasted) = mpsc::channel();
        let (transition, transitions) = mpsc::channel();

        let foreground = front.clone();
        let polish_enabled = enabled.clone();
        let config = Config {
            app_rules: setup.rules,
            postprocess: setup.postprocess,
            append_space: false,
            dictionary: setup.dictionary,
            ..Config::default()
        };
        let dictation = Dictation::new(DictationParts {
            config,
            recorder: Box::new(FakeRecorder { started: started.clone() }),
            transcriber: transcriber.clone(),
            paste: Box::new(move |text| {
                let _ = paste.send(text.to_owned());
                Ok(())
            }),
            cues: Arc::new(Silent),
            on_transition: Box::new(move |t| {
                let _ = transition.send(t.clone());
            }),
            polisher: client,
            polish_enabled: Arc::new(move || polish_enabled.load(Ordering::SeqCst)),
            foreground_app: Box::new(move || foreground.lock().unwrap().clone()),
            // The tests post every release themselves, so the fallback for a release the hook missed stays out.
            hotkey_down: Box::new(|| true),
        });
        Self { front, started, transcriber, polisher: fake, pasted, transitions, enabled, dictation }
    }

    fn set_front(&self, app: Option<&str>) {
        *self.front.lock().unwrap() = app.map(str::to_owned);
    }

    fn polisher(&self) -> &FakePolisher {
        self.polisher.as_ref().expect("the default polisher")
    }

    fn begin(&self) {
        self.started.clear();
        self.dictation.post(Event::HotkeyDown(0.0));
        assert!(self.started.wait(WAIT));
    }

    fn pasted(&self) -> String {
        self.pasted.recv_timeout(WAIT).expect("a paste")
    }

    fn nothing_pasted(&self) -> bool {
        self.pasted.try_recv().is_err()
    }

    fn wait_state(&self, wanted: fn(&State) -> bool) {
        while !wanted(&self.transitions.recv_timeout(WAIT).expect("a transition").state) {}
    }
}

fn recording(state: &State) -> bool {
    matches!(state, State::Recording { .. })
}

fn polishing(state: &State) -> bool {
    matches!(state, State::Polishing { .. })
}

fn awaiting_release(state: &State) -> bool {
    matches!(state, State::AwaitingRelease { .. })
}

fn dictionary(words: &str, replacements: &str) -> PersonalDictionary {
    PersonalDictionary::from_text(words, replacements).unwrap()
}

fn timeout(seconds: f64) -> PostprocessConfig {
    PostprocessConfig { timeout_s: seconds, ..PostprocessConfig::default() }
}

#[test]
fn test_dictionary_does_not_enable_cloud_processing_when_toggle_is_off() {
    let h = Harness::start(Setup { rules: vec![], dictionary: dictionary("PI\nharness", "hi => PI"), ..Setup::default() });
    h.begin();
    h.dictation.post(Event::HotkeyUp(2.0));
    assert_eq!(h.pasted(), "PI");
    assert!(h.polisher().no_requests() && !h.polisher().preconnected.is_set());
}

#[test]
fn test_dictionary_is_applied_after_ai_cleanup_without_cascading() {
    let replacements = "hi => wrong\npolished => PI\nPI => do not cascade";
    let h = Harness::start(Setup { dictionary: dictionary("harness", replacements), ..Setup::default() });
    h.begin();
    h.dictation.post(Event::HotkeyUp(2.0));
    assert_eq!(h.pasted(), "PI");
    let (raw, instructions) = h.polisher().next_request();
    assert!(raw == "hi" && instructions.contains("Translate to English.") && instructions.contains("harness"));
}

#[derive(Clone, Copy, PartialEq)]
enum Skip {
    Escape,
    Timeout,
    Failure,
}

struct DelayedPolisher {
    skip: Skip,
    gate: Arc<Flag>,
}

impl Polish for DelayedPolisher {
    fn preconnect(&self) {}

    fn polish(&self, _raw: &str, _instructions: &str) -> Result<String, PolishError> {
        if self.skip == Skip::Failure {
            return Err(PolishError("test failure".to_owned()));
        }
        self.gate.wait(WAIT);
        Ok("late".to_owned())
    }
}

#[test]
fn test_dictionary_still_corrects_raw_fallback() {
    for skip in [Skip::Escape, Skip::Timeout, Skip::Failure] {
        let gate = Arc::new(Flag::default());
        let h = Harness::start(Setup {
            dictionary: dictionary("PI", "hi => PI"),
            polisher: Some(Arc::new(DelayedPolisher { skip, gate: gate.clone() })),
            postprocess: timeout(if skip == Skip::Timeout { 0.1 } else { 8.0 }),
            ..Setup::default()
        });
        h.begin();
        h.dictation.post(Event::HotkeyUp(2.0));
        h.wait_state(polishing);
        if skip == Skip::Escape {
            h.dictation.post(Event::Cancel);
        }
        assert_eq!(h.pasted(), "PI");
        gate.set();
    }
}

#[test]
fn test_rules_use_the_app_at_recording_end_and_do_not_follow_focus_during_transcription() {
    let h = Harness::start(Setup::default());
    h.set_front(Some("notepad.exe"));
    h.transcriber.release.clear();
    h.begin();
    h.set_front(Some("SLACK.EXE"));
    h.dictation.post(Event::HotkeyUp(2.0));
    assert!(h.transcriber.entered.wait(WAIT));
    h.set_front(Some("notepad.exe"));
    h.transcriber.release.set();
    assert_eq!(h.pasted(), "polished");
    assert_eq!(h.polisher().next_request(), ("hi".to_owned(), "Translate to English.".to_owned()));
    assert!(!h.enabled.load(Ordering::SeqCst));
}

#[test]
fn test_a_rule_does_not_leak_into_the_next_dictation_without_a_match() {
    let h = Harness::start(Setup::default());
    h.begin();
    h.dictation.post(Event::HotkeyUp(2.0));
    assert_eq!(h.pasted(), "polished");
    assert_eq!(h.polisher().next_request(), ("hi".to_owned(), "Translate to English.".to_owned()));
    h.set_front(Some("unknown.exe"));
    h.begin();
    h.dictation.post(Event::HotkeyUp(2.0));
    assert_eq!(h.pasted(), "hi");
    assert!(h.polisher().no_requests());
}

#[test]
fn test_unmatched_disabled_and_empty_rules_leave_raw_dictation_local_when_toggle_is_off() {
    for front in [None, Some("unmatched.exe"), Some("slack.exe")] {
        let rules =
            vec![AppRule::new("Off", "Translate.", &["slack.exe"]).disabled(), AppRule::new("Empty", " \n", &["slack.exe"])];
        let h = Harness::start(Setup { rules, ..Setup::default() });
        h.set_front(front);
        h.begin();
        h.dictation.post(Event::HotkeyUp(2.0));
        assert_eq!(h.pasted(), "hi");
        assert!(h.polisher().no_requests() && !h.polisher().preconnected.is_set());
    }
}

static LOGGED: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Caplog;

impl log::Log for Caplog {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        LOGGED.lock().unwrap().push(record.args().to_string());
    }

    fn flush(&self) {}
}

fn capture_log() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&Caplog).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    });
}

#[test]
fn test_a_matching_rule_without_a_provider_key_pastes_raw_and_explains_why() {
    capture_log();
    let h = Harness::start(Setup {
        with_key: false,
        postprocess: PostprocessConfig::for_provider(PolishProvider::Anthropic),
        ..Setup::default()
    });
    h.begin();
    h.dictation.post(Event::HotkeyUp(2.0));
    assert_eq!(h.pasted(), "hi");
    let logged = LOGGED.lock().unwrap().join("\n");
    assert!(logged.contains("app rules need ANTHROPIC_API_KEY"));
    assert!(!logged.contains("Translate to English"));
}

struct ScriptedTransport {
    provider: PolishProvider,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Transport for ScriptedTransport {
    fn options(&self, _url: &str, _headers: &Headers) -> Result<Reply, TransportError> {
        Ok(Reply { status: 200, body: Vec::new() })
    }

    fn post(&self, _url: &str, _headers: &Headers, body: &[u8]) -> Result<Reply, TransportError> {
        self.requests.lock().unwrap().push(serde_json::from_slice(body).unwrap());
        let reply = match self.provider {
            PolishProvider::Anthropic => json!({"content": [{"type": "text", "text": "Hello."}], "stop_reason": "end_turn"}),
            PolishProvider::Zai => json!({"choices": [{"message": {"content": "Hello."}, "finish_reason": "stop"}]}),
        };
        Ok(Reply { status: 200, body: serde_json::to_vec(&reply).unwrap() })
    }
}

#[test]
fn test_local_rules_reach_the_selected_client_even_with_postprocessing_off() {
    for provider in [PolishProvider::Zai, PolishProvider::Anthropic] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let config = PostprocessConfig::for_provider(provider);
        let transport = ScriptedTransport { provider, requests: requests.clone() };
        let client = create_polisher(&config, "test-key", Some(Box::new(transport)));
        let h = Harness::start(Setup { polisher: Some(Arc::new(client)), postprocess: config, ..Setup::default() });
        h.begin();
        h.dictation.post(Event::HotkeyUp(2.0));
        assert_eq!(h.pasted(), "Hello.");
        let requests = requests.lock().unwrap();
        let [body] = requests.as_slice() else { panic!("{} requests", requests.len()) };
        let system = if provider == PolishProvider::Anthropic { &body["system"] } else { &body["messages"][0]["content"] };
        assert!(system.as_str().unwrap().ends_with("\n\nTranslate to English."));
    }
}

#[test]
fn test_a_rule_in_hands_free_mode_still_waits_for_hotkey_release() {
    let h = Harness::start(Setup::default());
    h.begin();
    h.wait_state(recording);
    h.dictation.post(Event::HotkeyUp(0.1));
    h.wait_state(recording);
    h.dictation.post(Event::HotkeyDown(2.0));
    h.wait_state(awaiting_release);
    assert!(h.nothing_pasted());
    h.dictation.post(Event::HotkeyUp(3.0));
    assert_eq!(h.pasted(), "polished");
}

/// The Python test blocks both workers of the polish pool to queue its requests. Rust gives each request
/// a thread, so the requests wait here instead, past the moment their instructions were captured.
#[derive(Default)]
struct BlockedPolisher {
    requests: Mutex<Vec<Request>>,
    release: Flag,
}

impl Polish for BlockedPolisher {
    fn preconnect(&self) {}

    fn polish(&self, raw: &str, instructions: &str) -> Result<String, PolishError> {
        assert!(self.release.wait(WAIT), "polishing was not released");
        self.requests.lock().unwrap().push((raw.to_owned(), instructions.to_owned()));
        Ok("polished".to_owned())
    }
}

fn wait_until(done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < WAIT, "gave up waiting");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn test_queued_requests_keep_their_rules_after_raw_fallback_and_a_new_dictation() {
    for skip in [Skip::Escape, Skip::Timeout] {
        let polisher = Arc::new(BlockedPolisher::default());
        let h = Harness::start(Setup {
            polisher: Some(polisher.clone()),
            postprocess: timeout(if skip == Skip::Timeout { 0.5 } else { 8.0 }),
            ..Setup::default()
        });
        h.begin();
        h.dictation.post(Event::HotkeyUp(2.0));
        h.wait_state(polishing);
        if skip == Skip::Escape {
            h.dictation.post(Event::Cancel);
        }
        assert_eq!(h.pasted(), "hi");

        h.set_front(Some("notepad.exe"));
        h.begin();
        h.dictation.post(Event::HotkeyUp(2.0));
        h.wait_state(polishing);
        polisher.release.set();
        assert_eq!(h.pasted(), "polished");
        // Each request's thread drops its handle after posting the reply; this one and the controller's remain.
        wait_until(|| Arc::strong_count(&polisher) == 2);
        let mut requests = polisher.requests.lock().unwrap().clone();
        requests.sort();
        assert_eq!(
            requests,
            [("hi".to_owned(), "Be formal.".to_owned()), ("hi".to_owned(), "Translate to English.".to_owned())]
        );
        // A new recording starts only after the controller consumes every late reply above.
        h.begin();
        assert!(h.nothing_pasted());
    }
}

#[test]
fn test_applying_settings_waits_for_recording_to_finish_and_text_to_be_pasted() {
    let h = Harness::start(Setup::default());
    h.begin();
    let finished = Flag::default();
    thread::scope(|scope| {
        scope.spawn(|| {
            h.dictation.finish();
            finished.set();
        });
        assert!(!finished.wait(BRIEFLY));
        h.dictation.post(Event::HotkeyUp(2.0));
        assert_eq!(h.pasted(), "polished");
        assert!(finished.wait(WAIT));
    });
    // Python asserts the controller thread ended. Here a press after finish must start nothing.
    h.started.clear();
    h.dictation.post(Event::HotkeyDown(4.0));
    assert!(!h.started.wait(BRIEFLY));
}

#[test]
fn test_applying_settings_also_waits_for_a_held_hotkey() {
    let h = Harness::start(Setup::default());
    h.begin();
    h.wait_state(recording);
    h.dictation.post(Event::HotkeyUp(0.1));
    h.wait_state(recording);
    h.dictation.post(Event::HotkeyDown(2.0));
    h.wait_state(awaiting_release);
    let finished = Flag::default();
    thread::scope(|scope| {
        scope.spawn(|| {
            h.dictation.finish();
            finished.set();
        });
        assert!(!finished.wait(BRIEFLY) && h.nothing_pasted());
        h.dictation.post(Event::HotkeyUp(3.0));
        assert_eq!(h.pasted(), "polished");
        assert!(finished.wait(WAIT));
    });
}
