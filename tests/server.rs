//! Ported from tests/test_server.py: the server for Mac, over real sockets on localhost.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use parakeet_dictate::config::{Config, PolishProvider, PostprocessConfig, ServerConfig};
use parakeet_dictate::cues::PlayCue;
use parakeet_dictate::dictation::{Dictation, DictationParts};
use parakeet_dictate::polish::{Headers, Polish, PolishError, Reply, Transport, TransportError, create_polisher};
use parakeet_dictate::recorder::{Audio, Record};
use parakeet_dictate::reducer::{Cue, Event, Transition};
use parakeet_dictate::server::{DictationServer, MAX_BODY_BYTES, ServerModel, ServerParts};
use parakeet_dictate::transcriber::{LoadedModel, SharedModel};
use serde_json::{Value, json};
use tempfile::TempDir;

const RAW: &str = "um so the meeting is at three";
const POLISHED: &str = "So the meeting is at three.";

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

    fn is_set(&self) -> bool {
        *self.set.lock().unwrap()
    }

    fn wait(&self, seconds: u64) -> bool {
        let set = self.set.lock().unwrap();
        *self.changed.wait_timeout_while(set, Duration::from_secs(seconds), |set| !*set).unwrap().0
    }
}

/// Records when each transcription entered and left, to prove two never overlap.
struct FakeTranscriber {
    text: String,
    failure: Option<String>,
    seconds: f64,
    audio: Mutex<Vec<Audio>>,
    spans: Mutex<Vec<(Instant, Instant)>>,
    entered: Flag,
}

impl FakeTranscriber {
    fn new(text: &str, seconds: f64) -> Arc<Self> {
        Arc::new(Self {
            text: text.to_owned(),
            failure: None,
            seconds,
            audio: Mutex::default(),
            spans: Mutex::default(),
            entered: Flag::default(),
        })
    }

    fn default_speech() -> Arc<Self> {
        Self::new(RAW, 0.0)
    }

    fn failing(message: &str) -> Arc<Self> {
        Arc::new(Self {
            text: String::new(),
            failure: Some(message.to_owned()),
            seconds: 0.0,
            audio: Mutex::default(),
            spans: Mutex::default(),
            entered: Flag::default(),
        })
    }

    fn heard_nothing(&self) -> bool {
        self.audio.lock().unwrap().is_empty()
    }
}

struct Loaded(Arc<FakeTranscriber>);

impl LoadedModel for Loaded {
    fn transcribe(&mut self, audio: &Audio) -> Result<String, String> {
        let fake = &self.0;
        let enter = Instant::now();
        fake.entered.set();
        thread::sleep(Duration::from_secs_f64(fake.seconds));
        fake.audio.lock().unwrap().push(audio.clone());
        fake.spans.lock().unwrap().push((enter, Instant::now()));
        match &fake.failure {
            Some(message) => Err(message.clone()),
            None => Ok(fake.text.clone()),
        }
    }
}

fn shared_model(transcriber: &Arc<FakeTranscriber>) -> Arc<SharedModel> {
    let model = SharedModel::default();
    model.load(Box::new(Loaded(transcriber.clone())));
    Arc::new(model)
}

enum Fails {
    Never,
    With(PolishError),
    Panics,
}

struct FakeGlm {
    fails: Fails,
    seconds: f64,
    polished: Mutex<Vec<String>>,
    instructions: Mutex<Vec<String>>,
    preconnected: Flag,
}

impl FakeGlm {
    fn new(fails: Fails, seconds: f64) -> Arc<Self> {
        Arc::new(Self { fails, seconds, polished: Mutex::default(), instructions: Mutex::default(), preconnected: Flag::default() })
    }

    fn working() -> Arc<Self> {
        Self::new(Fails::Never, 0.0)
    }

    fn polished(&self) -> Vec<String> {
        self.polished.lock().unwrap().clone()
    }
}

impl Polish for FakeGlm {
    fn preconnect(&self) {
        self.preconnected.set();
    }

    fn polish(&self, raw: &str, instructions: &str) -> Result<String, PolishError> {
        self.polished.lock().unwrap().push(raw.to_owned());
        self.instructions.lock().unwrap().push(instructions.to_owned());
        thread::sleep(Duration::from_secs_f64(self.seconds));
        match &self.fails {
            Fails::Never => Ok(POLISHED.to_owned()),
            Fails::With(error) => Err(error.clone()),
            Fails::Panics => panic!("boom"),
        }
    }
}

/// What the fixture's `start` takes as keyword arguments.
struct Setup {
    transcriber: Option<Arc<FakeTranscriber>>,
    glm: Option<Arc<dyn Polish>>,
    postprocess: bool,
    timeout_s: f64,
    model: Option<Arc<dyn ServerModel>>,
}

impl Default for Setup {
    fn default() -> Self {
        Self { transcriber: None, glm: None, postprocess: true, timeout_s: 2.0, model: None }
    }
}

/// The `serve` fixture: each test's temporary token file, and its servers, stopped at the end.
struct Serve {
    directory: TempDir,
    servers: Mutex<Vec<Arc<DictationServer>>>,
}

impl Serve {
    fn new() -> Self {
        Self { directory: TempDir::new().unwrap(), servers: Mutex::default() }
    }

    fn start(&self, setup: Setup) -> Arc<DictationServer> {
        let model = setup.model.unwrap_or_else(|| {
            let transcriber = setup.transcriber.unwrap_or_else(FakeTranscriber::default_speech);
            shared_model(&transcriber)
        });
        let postprocess = setup.postprocess;
        let server = DictationServer::new(ServerParts {
            config: ServerConfig { bind: "127.0.0.1".to_owned(), port: 0, ..ServerConfig::default() },
            model,
            polisher: setup.glm,
            polish_enabled: Arc::new(move || postprocess),
            timeout_s: setup.timeout_s,
            token_path: self.directory.path().join("server_token.txt"),
        });
        server.start().unwrap();
        let server = Arc::new(server);
        self.servers.lock().unwrap().push(server.clone());
        server
    }

    fn with_transcriber(&self, transcriber: &Arc<FakeTranscriber>) -> Arc<DictationServer> {
        self.start(Setup { transcriber: Some(transcriber.clone()), ..Setup::default() })
    }

    fn with_glm(&self, glm: &Arc<FakeGlm>) -> Arc<DictationServer> {
        self.start(Setup { glm: Some(glm.clone()), ..Setup::default() })
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        for server in self.servers.lock().unwrap().iter() {
            server.stop();
        }
    }
}

fn wav(samples: &[i16], rate: u32, channels: u16) -> Vec<u8> {
    let frames: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
    let block = channels * 2;
    let mut format = Vec::new();
    format.extend_from_slice(&1u16.to_le_bytes());
    format.extend_from_slice(&channels.to_le_bytes());
    format.extend_from_slice(&rate.to_le_bytes());
    format.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    format.extend_from_slice(&block.to_le_bytes());
    format.extend_from_slice(&16u16.to_le_bytes());
    riff(&format, &frames)
}

fn wav_header(format_tag: u16, bits: u16, frames: &[u8]) -> Vec<u8> {
    const RATE: u32 = 16_000;
    let block = bits / 8;
    let mut format = Vec::new();
    format.extend_from_slice(&format_tag.to_le_bytes());
    format.extend_from_slice(&1u16.to_le_bytes());
    format.extend_from_slice(&RATE.to_le_bytes());
    format.extend_from_slice(&(RATE * u32::from(block)).to_le_bytes());
    format.extend_from_slice(&block.to_le_bytes());
    format.extend_from_slice(&bits.to_le_bytes());
    riff(&format, frames)
}

fn riff(format: &[u8], frames: &[u8]) -> Vec<u8> {
    let mut file = b"RIFF".to_vec();
    file.extend_from_slice(&((4 + 8 + format.len() + 8 + frames.len()) as u32).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&(format.len() as u32).to_le_bytes());
    file.extend_from_slice(format);
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    file.extend_from_slice(frames);
    file
}

fn speech() -> Vec<u8> {
    wav(&vec![1000; 16_000], 16_000, 1)
}

struct Response {
    status: u16,
    body: Vec<u8>,
}

impl Response {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    fn message(&self) -> String {
        self.json()["error"]["message"].as_str().unwrap().to_owned()
    }
}

/// Reads to the close, since every reply closes its connection. No bytes means no reply at all.
fn read_reply(stream: &mut TcpStream) -> Vec<u8> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    reply
}

/// Parses a reply and checks what docs/mac-client.md promises of every one.
fn parse_reply(reply: &[u8]) -> Response {
    let split = reply.windows(4).position(|window| window == b"\r\n\r\n").expect("a complete head");
    let head = str::from_utf8(&reply[..split]).unwrap();
    let body = reply[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(&str, &str)> = lines.map(|line| line.split_once(": ").unwrap()).collect();
    let header = |name: &str| headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| *value);
    assert_eq!(header("Content-Type"), Some("application/json; charset=utf-8"));
    assert_eq!(header("Connection"), Some("close"));
    assert_eq!(header("Content-Length"), Some(body.len().to_string().as_str()));
    Response { status, body }
}

fn send(server: &DictationServer, request: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", server.port().unwrap())).unwrap();
    stream.write_all(request).unwrap();
    read_reply(&mut stream)
}

/// The server's own token by default; "" sends no Authorization header.
fn auth(server: &DictationServer, token: Option<&str>) -> String {
    match token {
        Some("") => String::new(),
        Some(token) => format!("Authorization: Bearer {token}\r\n"),
        None => format!("Authorization: Bearer {}\r\n", server.token()),
    }
}

const BOUNDARY: &str = "parakeet-test-boundary";

fn form(parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, filename, value) in parts {
        let filename = filename.map_or_else(String::new, |filename| format!("; filename=\"{filename}\""));
        let kind = if filename.is_empty() { "" } else { "Content-Type: audio/wav\r\n" };
        let head = format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"{filename}\r\n{kind}\r\n");
        body.extend_from_slice(head.as_bytes());
        body.extend_from_slice(value);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn post_form_raw(server: &DictationServer, token: Option<&str>, parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let body = form(parts);
    let mut request = format!(
        "POST /v1/audio/transcriptions HTTP/1.1\r\nHost: 127.0.0.1\r\n{}Content-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: {}\r\n\r\n",
        auth(server, token),
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(&body);
    send(server, &request)
}

fn post(server: &DictationServer, audio: &[u8], token: Option<&str>, fields: &[(&str, &str)]) -> Response {
    let mut parts: Vec<(&str, Option<&str>, &[u8])> = fields.iter().map(|(name, value)| (*name, None, value.as_bytes())).collect();
    parts.push(("file", Some("speech.wav"), audio));
    parse_reply(&post_form_raw(server, token, &parts))
}

fn post_speech(server: &DictationServer, fields: &[(&str, &str)]) -> Response {
    post(server, &speech(), None, fields)
}

fn health(server: &DictationServer, token: Option<&str>) -> Response {
    let request = format!("GET /v1/health HTTP/1.1\r\nHost: 127.0.0.1\r\n{}\r\n", auth(server, token));
    parse_reply(&send(server, request.as_bytes()))
}

struct ScriptedClaude<F> {
    handler: F,
}

impl<F: Fn(&[u8]) -> Reply + Send + Sync> Transport for ScriptedClaude<F> {
    fn options(&self, _: &str, _: &Headers) -> Result<Reply, TransportError> {
        Ok(Reply { status: 200, body: Vec::new() })
    }

    fn post(&self, _: &str, _: &Headers, body: &[u8]) -> Result<Reply, TransportError> {
        Ok((self.handler)(body))
    }
}

#[test]
fn test_server_uses_claude_for_mac_instructions_and_preserves_raw_fallback() {
    for reason in ["end_turn", "refusal", "max_tokens"] {
        let serve = Serve::new();
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let requests = seen.clone();
        let handler = move |body: &[u8]| {
            requests.lock().unwrap().push(serde_json::from_slice(body).unwrap());
            let stop_details = if reason == "refusal" { json!({"category": "cyber"}) } else { Value::Null };
            let payload = json!({
                "content": [{"type": "text", "text": POLISHED}], "stop_reason": reason, "stop_details": stop_details,
            });
            Reply { status: 200, body: serde_json::to_vec(&payload).unwrap() }
        };
        let config = PostprocessConfig::for_provider(PolishProvider::Anthropic);
        let client = create_polisher(&config, "test-key", Some(Box::new(ScriptedClaude { handler })));
        let server = serve.start(Setup { glm: Some(Arc::new(client)), postprocess: false, ..Setup::default() });
        let response = post_speech(&server, &[("postprocess", "true"), ("instructions", "Translate to English.")]);
        assert_eq!(response.status, 200);
        let finished = reason == "end_turn";
        assert_eq!(response.json(), json!({"text": if finished { POLISHED } else { RAW }, "raw": RAW, "polished": finished}));
        {
            let seen = seen.lock().unwrap();
            assert!(seen[0]["system"].as_str().unwrap().ends_with("\n\nTranslate to English."));
            assert_eq!(seen[0]["model"], "claude-haiku-5-5");
        }
        let unpolished = post_speech(&server, &[("postprocess", "false"), ("instructions", "Be formal.")]);
        assert_eq!(unpolished.json()["polished"], false);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}

#[test]
fn test_the_token_is_created_once_and_kept() {
    let serve = Serve::new();
    let first = serve.start(Setup::default());
    let token = fs::read_to_string(serve.directory.path().join("server_token.txt")).unwrap();
    assert_eq!(first.token(), token);
    assert!(token.len() >= 40);
    assert_eq!(serve.start(Setup::default()).token(), token);
}

#[test]
fn test_a_missing_or_wrong_token_gets_401_and_transcribes_nothing() {
    for template in ["", "wrong", "{token}x"] {
        let serve = Serve::new();
        let transcriber = FakeTranscriber::default_speech();
        let server = serve.with_transcriber(&transcriber);
        let token = template.replace("{token}", &server.token());
        assert_eq!(health(&server, Some(&token)).status, 401);
        let response = post(&server, &speech(), Some(&token), &[]);
        assert_eq!(response.status, 401);
        assert_eq!(response.json(), json!({"error": {"message": "missing or wrong bearer token"}}));
        assert!(transcriber.heard_nothing());
    }
}

#[test]
fn test_audio_that_is_not_16_bit_pcm_wav_gets_400_naming_the_format() {
    let cases = [
        ("not-wav", b"not a wav at all".to_vec()),
        ("pcm24", wav_header(1, 24, &vec![0; 3 * 1600])),
        ("float32", wav_header(3, 32, &vec![0; 4 * 1600])),
    ];
    for (id, audio) in cases {
        let serve = Serve::new();
        let transcriber = FakeTranscriber::default_speech();
        let response = post(&serve.with_transcriber(&transcriber), &audio, None, &[]);
        assert_eq!(response.status, 400, "{id}");
        assert!(response.message().contains("16-bit PCM"), "{id}");
        assert!(transcriber.heard_nothing(), "{id}");
    }
}

#[test]
fn test_a_wav_comes_back_transcribed_and_polished() {
    let serve = Serve::new();
    let glm = FakeGlm::working();
    let response = post_speech(&serve.with_glm(&glm), &[]);
    assert_eq!(response.status, 200);
    assert_eq!(response.json(), json!({"text": POLISHED, "raw": RAW, "polished": true}));
    assert_eq!(glm.polished(), [RAW]);
}

#[test]
fn test_stereo_at_any_rate_reaches_the_model_as_mono_at_that_rate() {
    let serve = Serve::new();
    let transcriber = FakeTranscriber::default_speech();
    let stereo: Vec<i16> = [3000, 1000].repeat(44_100);
    let audio = wav(&stereo, 44_100, 2);
    assert_eq!(post(&serve.with_transcriber(&transcriber), &audio, None, &[("postprocess", "false")]).status, 200);
    let heard = transcriber.audio.lock().unwrap();
    let [audio] = heard.as_slice() else { panic!("expected one transcription") };
    assert_eq!((audio.rate, audio.samples.len()), (44_100, 44_100));
    let expected = 2000.0 / 32768.0_f32;
    assert!(audio.samples.iter().all(|sample| (sample - expected).abs() <= 1e-8 + expected * 1e-5));
}

#[test]
fn test_postprocess_false_skips_glm() {
    let serve = Serve::new();
    let glm = FakeGlm::working();
    let response = post_speech(&serve.with_glm(&glm), &[("postprocess", "false")]);
    assert_eq!(response.json(), json!({"text": RAW, "raw": RAW, "polished": false}));
    assert!(glm.polished().is_empty());
}

#[test]
fn test_postprocess_defaults_to_the_tray_choice() {
    let serve = Serve::new();
    let glm = FakeGlm::working();
    let off = |serve: &Serve| serve.start(Setup { glm: Some(glm.clone()), postprocess: false, ..Setup::default() });
    assert_eq!(post_speech(&off(&serve), &[]).json()["polished"], false);
    assert_eq!(post_speech(&off(&serve), &[("postprocess", "true")]).json()["polished"], true);
}

#[test]
fn test_instructions_reach_glm() {
    let serve = Serve::new();
    let glm = FakeGlm::working();
    let server = serve.with_glm(&glm);
    assert_eq!(post_speech(&server, &[("instructions", " Translate to English. ")]).json()["polished"], true);
    assert_eq!(post_speech(&server, &[]).json()["polished"], true);
    assert_eq!(*glm.instructions.lock().unwrap(), ["Translate to English.", ""]);
}

#[test]
fn test_instructions_that_are_not_utf8_get_400() {
    let serve = Serve::new();
    let glm = FakeGlm::working();
    let server = serve.with_glm(&glm);
    let audio = speech();
    let reply = post_form_raw(&server, None, &[("file", Some("speech.wav"), &audio), ("instructions", None, b"\xff\xfe")]);
    let response = parse_reply(&reply);
    assert_eq!(response.status, 400);
    assert!(response.message().contains("UTF-8"));
    assert!(glm.polished().is_empty());
}

#[test]
fn test_a_glm_failure_or_timeout_returns_the_raw_text() {
    let cases = [
        ("polish-error", FakeGlm::new(Fails::With(PolishError("HTTP 500".to_owned())), 0.0)),
        ("crash", FakeGlm::new(Fails::Panics, 0.0)),
        ("slower-than-timeout", FakeGlm::new(Fails::Never, 2.0)),
    ];
    for (id, glm) in cases {
        let serve = Serve::new();
        let server = serve.start(Setup { glm: Some(glm), timeout_s: 0.3, ..Setup::default() });
        let started = Instant::now();
        let response = post_speech(&server, &[]);
        assert_eq!(response.status, 200, "{id}");
        assert_eq!(response.json(), json!({"text": RAW, "raw": RAW, "polished": false}), "{id}");
        assert!(started.elapsed().as_secs_f64() < 1.5, "{id}");
    }
}

#[test]
fn test_empty_speech_returns_empty_text_without_glm() {
    let serve = Serve::new();
    let glm = FakeGlm::working();
    let server = serve.start(Setup { transcriber: Some(FakeTranscriber::new("", 0.0)), glm: Some(glm.clone()), ..Setup::default() });
    assert_eq!(post_speech(&server, &[]).json(), json!({"text": "", "raw": "", "polished": false}));
    assert!(glm.polished().is_empty());
}

#[test]
fn test_health_reports_postprocess_and_warms_glm_only_when_on() {
    let serve = Serve::new();
    let (on, off) = (FakeGlm::working(), FakeGlm::working());
    assert_eq!(health(&serve.with_glm(&on), None).json(), json!({"status": "ok", "postprocess": true}));
    assert!(on.preconnected.wait(5));
    let tray_off = serve.start(Setup { glm: Some(off.clone()), postprocess: false, ..Setup::default() });
    assert_eq!(health(&tray_off, None).json(), json!({"status": "ok", "postprocess": false}));
    assert_eq!(health(&serve.start(Setup::default()), None).json(), json!({"status": "ok", "postprocess": false}));
    thread::sleep(Duration::from_millis(100));
    assert!(!off.preconnected.is_set());
}

#[test]
fn test_503_while_the_model_loads() {
    let serve = Serve::new();
    let server = serve.start(Setup { model: Some(Arc::new(SharedModel::default())), ..Setup::default() });
    assert_eq!(post_speech(&server, &[]).status, 503);
}

#[test]
fn test_413_above_the_size_cap_without_reading_the_body() {
    let serve = Serve::new();
    let server = serve.start(Setup::default());
    let head = format!(
        "POST /v1/audio/transcriptions HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Type: multipart/form-data; boundary=x\r\nContent-Length: {}\r\n\r\n",
        server.token(),
        MAX_BODY_BYTES + 1
    );
    assert_eq!(parse_reply(&send(&server, head.as_bytes())).status, 413);
}

struct SecondOfSilence;

impl Record for SecondOfSilence {
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn stop(&mut self) -> Result<Audio, String> {
        Ok(Audio { samples: vec![0.0; 16_000], rate: 16_000 })
    }

    fn discard(&mut self) {}

    fn device_name(&self) -> String {
        "fake".to_owned()
    }
}

struct Silent;

impl PlayCue for Silent {
    fn play(&self, _: Cue) {}
}

#[test]
fn test_a_server_request_during_a_local_transcription_waits_for_it() {
    let serve = Serve::new();
    let transcriber = FakeTranscriber::new(RAW, 0.5);
    let model = shared_model(&transcriber);
    let pasted = Arc::new(Flag::default());
    let paste = pasted.clone();
    // Built without `listen`, so the real keyboard hook stays out of it.
    let dictation = Dictation::new(DictationParts {
        config: Config::default(),
        recorder: Box::new(SecondOfSilence),
        transcriber: model.clone(),
        paste: Box::new(move |_| {
            paste.set();
            Ok(())
        }),
        cues: Arc::new(Silent),
        on_transition: Box::new(|_: &Transition| {}),
        polisher: None,
        polish_enabled: Arc::new(|| false),
        foreground_app: Box::new(|| None),
        hotkey_down: Box::new(|| false),
    });
    let server = serve.start(Setup { model: Some(model), ..Setup::default() });
    dictation.post(Event::HotkeyDown(0.0));
    dictation.post(Event::HotkeyUp(2.0));
    assert!(transcriber.entered.wait(5));
    let response = post_speech(&server, &[("postprocess", "false")]);
    assert!(pasted.wait(5));
    dictation.stop();

    assert_eq!(response.json()["raw"], RAW);
    let spans = transcriber.spans.lock().unwrap();
    let [(_, local_out), (server_in, _)] = spans.as_slice() else { panic!("expected two transcriptions") };
    assert!(local_out <= server_in, "the server transcription overlapped the local one");
}

// Not in test_server.py, which never makes the model fail. In Python the exception leaves the handler
// and the connection closes with no reply, which the Mac reports as unreachable and paste nothing.
#[test]
fn a_failed_transcription_closes_the_connection_with_no_reply() {
    let serve = Serve::new();
    let glm = FakeGlm::working();
    let transcriber = FakeTranscriber::failing("the session failed");
    let server = serve.start(Setup { transcriber: Some(transcriber), glm: Some(glm.clone()), ..Setup::default() });
    let audio = speech();
    assert!(post_form_raw(&server, None, &[("file", Some("speech.wav"), &audio)]).is_empty());
    assert!(glm.polished().is_empty());
}
