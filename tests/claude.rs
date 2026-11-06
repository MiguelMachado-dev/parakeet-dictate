//! Ported from tests/test_claude.py: the Claude client, against a scripted transport.

use std::sync::{Arc, Mutex, Once};
use std::thread;
use std::time::Duration;

use log::{LevelFilter, Metadata, Record};
use parakeet_dictate::config::{PolishProvider, PostprocessConfig};
use parakeet_dictate::polish::{Headers, INSTRUCTIONS_PROMPT, Polish, Polisher, Reply, Transport, TransportError, create_polisher};
use serde_json::{Value, json};

const RAW: &str = "um so I think we should uh push the release";
const CLEAN: &str = "I think we should push the release.";

fn config() -> PostprocessConfig {
    PostprocessConfig { extra_instructions: "Names: João.".to_owned(), ..PostprocessConfig::for_provider(PolishProvider::Anthropic) }
}

#[derive(Clone)]
struct Seen {
    method: &'static str,
    url: String,
    headers: Headers,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

type Answer = Result<Reply, TransportError>;
type Requests = Arc<Mutex<Vec<Seen>>>;

/// The equivalent of httpx2.MockTransport.
struct Fake<F> {
    handler: F,
    seen: Requests,
}

impl<F: Fn(&Seen) -> Answer> Fake<F> {
    fn answer(&self, seen: Seen) -> Answer {
        self.seen.lock().unwrap().push(seen.clone());
        (self.handler)(&seen)
    }
}

impl<F: Fn(&Seen) -> Answer + Send + Sync> Transport for Fake<F> {
    fn options(&self, url: &str, headers: &Headers) -> Answer {
        self.answer(Seen { method: "OPTIONS", url: url.to_owned(), headers: headers.clone(), body: Vec::new() })
    }

    fn post(&self, url: &str, headers: &Headers, body: &[u8]) -> Answer {
        self.answer(Seen { method: "POST", url: url.to_owned(), headers: headers.clone(), body: body.to_vec() })
    }
}

fn response(status: u16, payload: &Value) -> Answer {
    Ok(Reply { status, body: serde_json::to_vec(payload).unwrap() })
}

fn text_response(status: u16, text: &str) -> Answer {
    Ok(Reply { status, body: text.as_bytes().to_vec() })
}

/// `extra` fields join the reply, and replace its fields of the same name.
fn reply_with(text: &str, reason: Option<&str>, extra: Value) -> Answer {
    let mut payload = json!({
        "content": [{"type": "thinking", "thinking": "private"}, {"type": "text", "text": text}],
        "stop_reason": reason, "usage": {"output_tokens": 12},
    });
    for (key, value) in extra.as_object().unwrap() {
        payload[key] = value.clone();
    }
    response(200, &payload)
}

fn reply() -> Answer {
    reply_with(CLEAN, Some("end_turn"), json!({}))
}

fn client_with(handler: impl Fn(&Seen) -> Answer + Send + Sync + 'static, config: &PostprocessConfig) -> (Polisher, Requests) {
    let seen = Requests::default();
    let transport = Fake { handler, seen: seen.clone() };
    (create_polisher(config, "test-key", Some(Box::new(transport))), seen)
}

fn client(handler: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> (Polisher, Requests) {
    client_with(handler, &config())
}

fn polish_error(handler: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> String {
    let message = client(handler).0.polish(RAW, "").unwrap_err().0;
    assert!(!message.contains(RAW) && !message.contains("push the release"));
    message
}

/// The equivalent of pytest's caplog: every record from every test in this binary, at any level.
static CAPTURED: Mutex<String> = Mutex::new(String::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        let mut captured = CAPTURED.lock().unwrap();
        captured.push_str(&record.args().to_string());
        captured.push('\n');
    }

    fn flush(&self) {}
}

fn capture_logs() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(LevelFilter::Trace);
    });
}

#[test]
fn test_selected_provider_sends_the_messages_api_contract() {
    // Python also asserts the type is ClaudeClient; Rust has one Polisher, so the request is the proof.
    let (claude, seen) = client(|_| reply());
    assert_eq!(claude.polish(RAW, "").unwrap(), CLEAN);
    let seen = seen.lock().unwrap();
    let [request] = seen.as_slice() else { panic!("expected one request") };
    let body = request.json();
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(request.header("x-api-key"), Some("test-key"));
    assert_eq!(request.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(request.header("authorization"), None);
    assert_eq!(request.header("anthropic-beta"), None);
    assert_eq!(body["model"], "claude-haiku-5-5");
    assert_eq!(body["max_tokens"], 16_000);
    assert_eq!(body["output_config"], json!({"effort": "low"}));
    assert_eq!(body["thinking"], json!({"type": "disabled"}));
    let system = body["system"].as_str().unwrap();
    assert!(system.starts_with("You clean up raw speech-to-text dictation."));
    assert!(system.ends_with("\n\nNames: João."));
    assert_eq!(body["messages"], json!([{"role": "user", "content": format!("<transcript>{RAW}</transcript>")}]));
    for absent in ["temperature", "stream", "fallbacks"] {
        assert!(body.get(absent).is_none(), "{absent} should not be sent");
    }
}

#[test]
fn test_thinking_and_fallback_are_specific_to_the_model() {
    for model in ["claude-haiku-5-5", "claude-sonnet-5-5"] {
        for thinking in [true, false] {
            let effort = if thinking { "max" } else { "high" };
            let config = PostprocessConfig { model: model.to_owned(), thinking, effort: effort.to_owned(), ..config() };
            let (claude, seen) = client_with(|_| reply(), &config);
            claude.polish(RAW, "").unwrap();
            let seen = seen.lock().unwrap();
            let [request] = seen.as_slice() else { panic!("expected one request") };
            let body = request.json();
            let sonnet = model == "claude-sonnet-5-5";
            let kind = match (thinking, sonnet) {
                (true, _) => "adaptive",
                (false, true) => "between_tools",
                (false, false) => "disabled",
            };
            assert_eq!(body["thinking"], json!({"type": kind}));
            assert_eq!(body["output_config"], json!({"effort": effort}));
            if sonnet {
                assert_eq!(request.header("anthropic-beta"), Some("server-side-fallback-2026-07-01"));
                assert_eq!(body["fallbacks"], "default");
            } else {
                assert_eq!(request.header("anthropic-beta"), None);
                assert!(body.get("fallbacks").is_none());
            }
        }
    }
}

#[test]
fn test_instructions_apply_to_one_request_without_changing_the_base_prompt() {
    let (claude, seen) = client(|_| reply());
    claude.polish(RAW, "  Translate to English.\n").unwrap();
    claude.polish(RAW, "").unwrap();
    let seen = seen.lock().unwrap();
    let prompts: Vec<String> = seen.iter().map(|request| request.json()["system"].as_str().unwrap().to_owned()).collect();
    let [with_rule, without_rule] = prompts.as_slice() else { panic!("expected two requests") };
    assert_eq!(*with_rule, format!("{without_rule}\n\n{INSTRUCTIONS_PROMPT}\n\nTranslate to English."));
    assert!(without_rule.ends_with("\n\nNames: João."));
}

#[test]
fn test_only_text_blocks_are_joined_and_cleaned() {
    let extra = json!({"content": [
        {"type": "thinking", "thinking": "private"},
        {"type": "text", "text": "<transcript>\" I think"},
        {"type": "text", "text": " so. \"</transcript>"},
    ]});
    let (claude, _) = client(move |_| reply_with(CLEAN, Some("end_turn"), extra.clone()));
    assert_eq!(claude.polish(RAW, "").unwrap(), "I think so.");
}

#[test]
fn test_refusal_reports_its_category_and_discards_partial_text() {
    let details = json!({"stop_details": {"category": "cyber", "explanation": RAW}});
    assert_eq!(polish_error(move |_| reply_with(RAW, Some("refusal"), details.clone())), "declined (category cyber)");
    assert_eq!(polish_error(|_| reply_with("", Some("refusal"), json!({"stop_details": null}))), "declined (category none)");
}

#[test]
fn test_incomplete_reply_falls_back_instead_of_delivering_partial_text() {
    for reason in [Some("max_tokens"), Some("tool_use"), Some("pause_turn"), Some("stop_sequence"), None] {
        assert!(polish_error(move |_| reply_with("partial", reason, json!({}))).contains("did not finish normally"));
    }
}

#[test]
fn test_malformed_reply_is_a_polish_error() {
    let payloads = [
        json!(null),
        json!([]),
        json!({}),
        json!({"content": null}),
        json!({"content": [null]}),
        json!({"content": [{"type": "text", "text": null}], "stop_reason": "end_turn"}),
    ];
    for payload in payloads {
        assert!(polish_error(move |_| response(200, &payload)).contains("unexpected reply shape"));
    }
}

#[test]
fn test_errors_keep_transcripts_keys_and_response_bodies_out_of_logs() {
    capture_logs();
    let message = polish_error(|_| {
        response(400, &json!({"error": {"type": "invalid_request_error", "message": format!("test-key: {RAW}")}}))
    });
    assert_eq!(message, "HTTP 400, invalid_request_error");
    assert_eq!(polish_error(|_| text_response(502, RAW)), "HTTP 502");
    assert!(polish_error(|_| text_response(200, RAW)).contains("unexpected reply shape"));
    // A success, so the capture is known to work: only this path logs.
    client(|_| reply()).0.polish(RAW, "").unwrap();
    let captured = CAPTURED.lock().unwrap();
    assert!(captured.contains("Claude replied"));
    assert!(!captured.contains(RAW) && !captured.contains("test-key") && !captured.contains(CLEAN));
}

#[test]
fn test_timeout_and_connection_errors_are_safe_to_log() {
    // httpx2's errors carry the transcript in their message here. TransportError cannot hold one.
    assert_eq!(polish_error(|_| Err(TransportError::Timeout)), "no reply within 8 s");
    assert_eq!(polish_error(|_| Err(TransportError::Failed("ConnectError".to_owned()))), "ConnectError");
}

#[test]
fn test_empty_or_overlong_text_is_rejected() {
    let overlong = "x".repeat(2 * RAW.chars().count() + 41);
    for text in ["", "   ", "\"\"", "<transcript></transcript>", overlong.as_str()] {
        let text = text.to_owned();
        polish_error(move |_| reply_with(&text, Some("end_turn"), json!({})));
    }
}

/// Python moves a fake clock; the Rust client reads `Instant`, so this one waits the real 15 s.
#[test]
fn test_preconnect_reuses_a_recent_connection_and_renews_after_fifteen_seconds() {
    let (claude, seen) = client(|request| if request.method == "POST" { reply() } else { text_response(200, "") });
    claude.preconnect();
    claude.preconnect();
    claude.polish(RAW, "").unwrap();
    thread::sleep(Duration::from_secs(14));
    claude.preconnect();
    thread::sleep(Duration::from_secs(1));
    claude.preconnect();
    let methods: Vec<&str> = seen.lock().unwrap().iter().map(|request| request.method).collect();
    assert_eq!(methods, ["OPTIONS", "POST", "OPTIONS"]);
}

#[test]
fn test_preconnect_failure_does_not_fail_a_recording() {
    client(|_| Err(TransportError::Failed("ConnectError".to_owned()))).0.preconnect();
}
