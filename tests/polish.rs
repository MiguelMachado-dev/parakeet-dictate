//! Ported from tests/test_polish.py: the GLM client, against a scripted transport.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use parakeet_dictate::config::PostprocessConfig;
use parakeet_dictate::polish::{Headers, INSTRUCTIONS_PROMPT, Polish, Polisher, Reply, Transport, TransportError, create_polisher};
use serde_json::{Value, json};

const RAW: &str = "um so I think we should uh push the release";

fn config() -> PostprocessConfig {
    PostprocessConfig {
        reasoning_effort: "high".to_owned(),
        extra_instructions: "Names: João.".to_owned(),
        ..PostprocessConfig::default()
    }
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
type Log = Arc<Mutex<Vec<Seen>>>;

/// The equivalent of httpx2.MockTransport.
struct Fake<F> {
    handler: F,
    seen: Log,
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

fn finished(content: &str, finish_reason: &str) -> Answer {
    response(
        200,
        &json!({
            "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": finish_reason}],
            "usage": {"completion_tokens": 12, "completion_tokens_details": {"reasoning_tokens": 3}},
        }),
    )
}

fn reply(content: &str) -> Answer {
    finished(content, "stop")
}

fn client(handler: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> (Polisher, Log) {
    let seen = Log::default();
    let transport = Fake { handler, seen: seen.clone() };
    (create_polisher(&config(), "test-key", Some(Box::new(transport))), seen)
}

fn polish_error(handler: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> String {
    let message = client(handler).0.polish(RAW, "").unwrap_err().0;
    assert!(!message.contains(RAW) && !message.contains("push the release"));
    message
}

#[test]
fn test_request_carries_auth_thinking_effort_and_the_tagged_transcript() {
    let (glm, seen) = client(|_| reply("I think we should push the release."));
    assert_eq!(glm.polish(RAW, "").unwrap(), "I think we should push the release.");
    let seen = seen.lock().unwrap();
    let [request] = seen.as_slice() else { panic!("expected one request") };
    let body = request.json();
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, config().endpoint);
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));
    assert_eq!(body["model"], "glm-5.3");
    assert_eq!(body["thinking"], json!({"type": "enabled"}));
    assert_eq!(body["reasoning_effort"], "high");
    assert_eq!(body["stream"], false);
    let messages = body["messages"].as_array().unwrap();
    let [system, user] = messages.as_slice() else { panic!("expected two messages") };
    assert_eq!(system["role"], "system");
    assert!(system["content"].as_str().unwrap().ends_with("\n\nNames: João."));
    assert_eq!(*user, json!({"role": "user", "content": format!("<transcript>{RAW}</transcript>")}));
}

#[test]
fn test_instructions_follow_the_system_prompt_for_one_dictation_only() {
    let (glm, seen) = client(|_| reply("I think we should push the release."));
    glm.polish(RAW, "  Translate to English.\n").unwrap();
    glm.polish(RAW, "").unwrap();
    let seen = seen.lock().unwrap();
    let prompts: Vec<String> = seen.iter().map(|request| request.json()["messages"][0]["content"].as_str().unwrap().to_owned()).collect();
    let [with_rule, without] = prompts.as_slice() else { panic!("expected two requests") };
    assert_eq!(*with_rule, format!("{without}\n\n{INSTRUCTIONS_PROMPT}\n\nTranslate to English."));
    assert!(without.ends_with("\n\nNames: João."));
}

#[test]
fn test_reply_is_stripped_of_whitespace_quotes_and_echoed_tags() {
    let cases = [
        ("  I think so.\n", "I think so."),
        ("\"I think so.\"", "I think so."),
        ("“I think so.”", "I think so."),
        ("<transcript>I think so.</transcript>", "I think so."),
        ("<transcript>\"I think so.\"</transcript>", "I think so."),
        ("\"<transcript> I think so. </transcript>\"", "I think so."),
        ("He said \"stop\" twice.", "He said \"stop\" twice."),
    ];
    for (content, expected) in cases {
        let content = content.to_owned();
        assert_eq!(client(move |_| reply(&content)).0.polish(RAW, "").unwrap(), expected);
    }
}

#[test]
fn test_timeout_is_a_polish_error() {
    assert_eq!(polish_error(|_| Err(TransportError::Timeout)), "no reply within 8 s");
}

#[test]
fn test_network_error_is_a_polish_error() {
    assert!(polish_error(|_| Err(TransportError::Failed("ConnectError".to_owned()))).contains("ConnectError"));
}

#[test]
fn test_http_error_reports_status_and_zai_code() {
    assert_eq!(
        polish_error(|_| response(429, &json!({"error": {"code": "1113", "message": "x"}}))),
        "HTTP 429, Z.ai error 1113"
    );
    assert_eq!(polish_error(|_| text_response(502, "bad gateway")), "HTTP 502");
}

#[test]
fn test_malformed_or_unfinished_reply_is_a_polish_error() {
    polish_error(|_| text_response(200, "not json"));
    polish_error(|_| response(200, &json!({"choices": []})));
    polish_error(|_| response(200, &json!({"choices": [{"message": {"content": null}, "finish_reason": "stop"}]})));
    polish_error(|_| finished("partial", "length"));
    polish_error(|_| finished("", "sensitive"));
}

#[test]
fn test_empty_reply_is_rejected() {
    for content in ["", "   ", "\"\"", "<transcript></transcript>"] {
        assert_eq!(polish_error(move |_| reply(content)), "empty reply");
    }
}

#[test]
fn test_reply_much_longer_than_the_transcript_is_rejected() {
    let limit = 2 * RAW.chars().count() + 40;
    let (fits, over) = ("x".repeat(limit), "x".repeat(limit + 1));
    let expected = fits.clone();
    assert_eq!(client(move |_| reply(&fits)).0.polish(RAW, "").unwrap(), expected);
    assert!(polish_error(move |_| reply(&over)).starts_with(&format!("reply of {} chars", limit + 1)));
}

#[test]
fn test_preconnect_opens_a_connection_only_when_none_is_kept_alive() {
    let (glm, seen) = client(|request| if request.method == "POST" { reply("ok") } else { text_response(200, "") });
    glm.preconnect();
    glm.preconnect();
    glm.polish(RAW, "").unwrap();
    glm.preconnect();
    let methods: Vec<&str> = seen.lock().unwrap().iter().map(|request| request.method).collect();
    assert_eq!(methods, ["OPTIONS", "POST"]);
}

#[test]
fn test_preconnect_failure_is_silent() {
    client(|_| Err(TransportError::Failed("ConnectError".to_owned()))).0.preconnect();
}

// The Python tests stop at httpx2.MockTransport, so what httpx did is pinned here for ureq, on localhost.

fn over_network(endpoint: String, timeout_s: f64) -> Polisher {
    let config = PostprocessConfig { endpoint, timeout_s, ..config() };
    create_polisher(&config, "test-key", None)
}

/// Answers one request with `answer`, then returns the request as text.
fn answer_once(answer: String) -> (u16, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 4096];
        while !complete(&request) {
            let read = stream.read(&mut chunk).unwrap();
            assert_ne!(read, 0, "the client went away mid-request");
            request.extend_from_slice(&chunk[..read]);
        }
        stream.write_all(answer.as_bytes()).unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });
    (port, server)
}

fn complete(request: &[u8]) -> bool {
    let text = String::from_utf8_lossy(request).to_lowercase();
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    let length = head.lines().find_map(|line| line.strip_prefix("content-length:")).map_or(0, |value| value.trim().parse().unwrap());
    body.len() >= length
}

#[test]
fn network_transport_posts_json_with_the_key() {
    let payload = json!({"choices": [{"message": {"content": "Push it."}, "finish_reason": "stop"}]}).to_string();
    let answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len());
    let (port, server) = answer_once(answer);
    let glm = over_network(format!("http://127.0.0.1:{port}/chat"), 5.0);
    assert_eq!(glm.polish(RAW, "").unwrap(), "Push it.");
    let request = server.join().unwrap().to_lowercase();
    assert!(request.starts_with("post /chat http/1.1\r\n"));
    assert!(request.contains("authorization: bearer test-key\r\n"));
    assert!(request.contains("content-type: application/json\r\n"));
}

#[test]
fn network_transport_reports_a_redirect_instead_of_following_it() {
    let answer = "HTTP/1.1 302 Found\r\nLocation: /elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned();
    let (port, server) = answer_once(answer);
    let glm = over_network(format!("http://127.0.0.1:{port}/chat"), 5.0);
    assert_eq!(glm.polish(RAW, "").unwrap_err().0, "HTTP 302");
    server.join().unwrap();
}

#[test]
fn network_transport_names_a_refused_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let glm = over_network(format!("http://127.0.0.1:{port}/chat"), 5.0);
    assert_eq!(glm.polish(RAW, "").unwrap_err().0, "ConnectError");
}

#[test]
fn network_transport_times_out_on_a_silent_server() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let glm = over_network(format!("http://127.0.0.1:{port}/chat"), 0.2);
    assert_eq!(glm.polish(RAW, "").unwrap_err().0, "no reply within 0.2 s");
}
