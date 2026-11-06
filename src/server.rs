//! The server for Mac: an HTTP API that lets another machine dictate through this PC's models.
//! docs/mac-client.md is the contract.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use log::{info, warn};
use serde_json::{Value, json};
use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::clock;
use crate::config::ServerConfig;
use crate::paths::temporary_beside;
use crate::polish::Polish;
use crate::recorder::Audio;
use crate::text::short_float;
use crate::transcriber::Transcribe;

pub const HEALTH_PATH: &str = "/v1/health";
pub const TRANSCRIPTIONS_PATH: &str = "/v1/audio/transcriptions";
// Ten minutes of 48 kHz 16-bit stereo, plus room for the multipart framing.
pub const MAX_BODY_BYTES: u64 = 10 * 60 * 48_000 * 2 * 2 + 64 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(60);
pub const EXPECTED_AUDIO: &str = "a WAV file of 16-bit PCM samples, mono or stereo, at any sample rate";
const LAN_QUERY: &str = r"
$up = @(Get-NetAdapter -Physical | Where-Object Status -eq Up).ifIndex
$routes = @(Get-NetRoute -AddressFamily IPv4 -DestinationPrefix 0.0.0.0/0 -ErrorAction SilentlyContinue)
ConvertTo-Json -Compress -InputObject @(Get-NetIPAddress -AddressFamily IPv4 | Where-Object InterfaceIndex -in $up | ForEach-Object {
    $index = $_.InterfaceIndex
    @{ address = $_.IPAddress; route_metric = ($routes | Where-Object ifIndex -eq $index | Measure-Object RouteMetric -Minimum).Minimum }
})
";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadRequest(pub String);

/// The readiness and transcription the server needs from the shared model.
pub trait ServerModel: Transcribe {
    fn ready(&self) -> bool;
}

impl ServerModel for crate::transcriber::SharedModel {
    fn ready(&self) -> bool {
        Self::ready(self)
    }
}

pub struct ServerParts {
    pub config: ServerConfig,
    pub model: Arc<dyn ServerModel>,
    pub polisher: Option<Arc<dyn Polish>>,
    pub polish_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    pub timeout_s: f64,
    pub token_path: PathBuf,
}

struct App {
    parts: ServerParts,
    token: Mutex<String>,
}

struct Listening {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// Started and stopped from the tray. Each request runs on its own thread; transcriptions still run
/// one at a time through the shared model.
pub struct DictationServer {
    app: Arc<App>,
    listening: Mutex<Option<Listening>>,
}

impl DictationServer {
    pub fn new(parts: ServerParts) -> Self {
        Self { app: Arc::new(App { parts, token: Mutex::new(String::new()) }), listening: Mutex::new(None) }
    }

    pub fn running(&self) -> bool {
        self.listening.lock().unwrap().is_some()
    }

    pub fn port(&self) -> Option<u16> {
        self.listening.lock().unwrap().as_ref().map(|listening| listening.address.port())
    }

    pub fn token(&self) -> String {
        self.app.token.lock().unwrap().clone()
    }

    pub fn polisher(&self) -> Option<&Arc<dyn Polish>> {
        self.app.parts.polisher.as_ref()
    }

    pub fn polish_on(&self) -> bool {
        self.app.polish_on()
    }

    /// Serve from now on. Fails when the port cannot be bound, and stays stopped.
    pub fn start(&self) -> io::Result<()> {
        let mut listening = self.listening.lock().unwrap();
        if listening.is_some() {
            return Ok(());
        }
        *self.app.token.lock().unwrap() = read_token(&self.app.parts.token_path)?;
        let config = &self.app.parts.config;
        // No SO_REUSEADDR: on Windows two sockets that both set it share the port, so a port held by another
        // such server would bind without error and the two would split the connections.
        let listener = TcpListener::bind((config.bind.as_str(), config.port))?;
        let address = listener.local_addr()?;
        let stopping = Arc::new(AtomicBool::new(false));
        let app = self.app.clone();
        let flag = stopping.clone();
        let thread = thread::Builder::new().name("server".to_owned()).spawn(move || serve(&listener, &app, &flag))?;
        info!("server for Mac listening on {}:{}", config.bind, address.port());
        *listening = Some(Listening { address, stopping, thread });
        Ok(())
    }

    pub fn stop(&self) {
        let Some(listening) = self.listening.lock().unwrap().take() else {
            return;
        };
        listening.stopping.store(true, Ordering::Release);
        // A connection wakes the blocking accept; the loop then sees the flag.
        let local = match listening.address.ip() {
            IpAddr::V4(address) if address.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
            other => other,
        };
        let _ = TcpStream::connect_timeout(&SocketAddr::new(local, listening.address.port()), Duration::from_secs(1));
        let _ = listening.thread.join();
        info!("server for Mac stopped");
    }

    /// The URL and the token, one per line, or None when this PC has no address on the local network
    /// or the server stopped meanwhile.
    pub fn connection_info(&self) -> Option<String> {
        self.connection_info_from(lan_ipv4)
    }

    pub fn connection_info_from(&self, find_address: impl FnOnce() -> Option<String>) -> Option<String> {
        let bind = &self.app.parts.config.bind;
        let address = if bind == "0.0.0.0" { find_address()? } else { bind.clone() };
        let port = self.port()?;
        Some(format!("http://{address}:{port}\n{}", self.token()))
    }
}

fn serve(listener: &TcpListener, app: &Arc<App>, stopping: &AtomicBool) {
    for stream in listener.incoming() {
        if stopping.load(Ordering::Acquire) {
            return;
        }
        let Ok(stream) = stream else {
            continue;
        };
        let app = app.clone();
        let handled = thread::Builder::new().name("server-request".to_owned()).spawn(move || {
            if let Err(e) = handle_connection(stream, &app) {
                warn!("server for Mac: {}", e.kind());
            }
        });
        if let Err(e) = handled {
            warn!("server for Mac: could not start a request thread: {e}");
        }
    }
}

struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }

    fn path(&self) -> &str {
        self.target.split(['?', '#']).next().unwrap_or("")
    }
}

/// The request line and headers, or None when the client went away before sending them.
fn read_head(reader: &mut BufReader<TcpStream>) -> io::Result<Option<Request>> {
    let mut lines = Vec::new();
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(None);
        }
        total += line.len();
        if total > MAX_HEADER_BYTES {
            return Err(io::ErrorKind::InvalidData.into());
        }
        while matches!(line.last(), Some(b'\r' | b'\n')) {
            line.pop();
        }
        if line.is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        lines.push(line.iter().map(|byte| char::from(*byte)).collect::<String>());
    }
    let mut first = lines[0].split_whitespace();
    let (Some(method), Some(target)) = (first.next(), first.next()) else {
        return Err(io::ErrorKind::InvalidData.into());
    };
    let headers = lines[1..]
        .iter()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    Ok(Some(Request { method: method.to_owned(), target: target.to_owned(), headers }))
}

#[derive(Default)]
struct Metrics {
    audio_s: Option<f64>,
    asr_ms: Option<f64>,
    polish_ms: Option<f64>,
}

fn shown(value: Option<f64>, digits: usize) -> String {
    value.map_or_else(|| "-".to_owned(), |value| format!("{value:.digits$}"))
}

fn reply(stream: &mut TcpStream, request: &Request, status: u16, payload: &Value, metrics: &Metrics) -> io::Result<()> {
    let body = serde_json::to_vec(payload).expect("a reply serializes");
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        411 => "Length Required",
        413 => "Payload Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Unknown",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&body)?;
    info!(
        "{} {} -> {status}, audio {} s, ASR {} ms, polish {} ms",
        request.method,
        request.path(),
        shown(metrics.audio_s, 1),
        shown(metrics.asr_ms, 0),
        shown(metrics.polish_ms, 0),
    );
    Ok(())
}

fn refuse(stream: &mut TcpStream, request: &Request, status: u16, message: &str) -> io::Result<()> {
    reply(stream, request, status, &json!({"error": {"message": message}}), &Metrics::default())
}

fn handle_connection(mut stream: TcpStream, app: &App) -> io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let Some(request) = read_head(&mut reader)? else {
        return Ok(());
    };
    let no_endpoint = format!("no such endpoint; GET {HEALTH_PATH} or POST {TRANSCRIPTIONS_PATH}");
    let wrong_token = "missing or wrong bearer token";
    match request.method.as_str() {
        "GET" => {
            if !app.authorized(request.header("Authorization").unwrap_or("")) {
                refuse(&mut stream, &request, 401, wrong_token)
            } else if request.path() != HEALTH_PATH {
                refuse(&mut stream, &request, 404, &no_endpoint)
            } else {
                reply(&mut stream, &request, 200, &app.health(), &Metrics::default())
            }
        }
        "POST" => {
            let length = request.header("Content-Length").and_then(|value| value.parse::<u64>().ok());
            let Some(length) = length else {
                return refuse(&mut stream, &request, 411, "send a Content-Length header");
            };
            if length > MAX_BODY_BYTES {
                let message = format!("the body is over {MAX_BODY_BYTES} bytes, about ten minutes of 48 kHz stereo");
                return refuse(&mut stream, &request, 413, &message);
            }
            // curl sends it for bodies over 1 MB and waits for this line before the body.
            if request.header("Expect").is_some_and(|value| value.eq_ignore_ascii_case("100-continue")) {
                stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            }
            // Read the body even to refuse it: closing a socket with unread data resets the connection on
            // Windows, and the client would see the reset instead of the status.
            let mut body = Vec::with_capacity(length as usize);
            (&mut reader).take(length).read_to_end(&mut body)?;
            if !app.authorized(request.header("Authorization").unwrap_or("")) {
                return refuse(&mut stream, &request, 401, wrong_token);
            }
            if request.path() != TRANSCRIPTIONS_PATH {
                return refuse(&mut stream, &request, 404, &no_endpoint);
            }
            if !app.parts.model.ready() {
                return refuse(&mut stream, &request, 503, "the model is still loading; try again in a few seconds");
            }
            let content_type = request.header("Content-Type").unwrap_or("");
            match parse_transcription(content_type, &body) {
                Err(BadRequest(message)) => refuse(&mut stream, &request, 400, &message),
                Ok((audio, postprocess, instructions)) => {
                    drop(body);
                    let mut metrics = Metrics { audio_s: Some(audio.seconds()), ..Metrics::default() };
                    match app.transcribe(&audio, postprocess, &instructions, &mut metrics) {
                        Ok(result) => reply(&mut stream, &request, 200, &result, &metrics),
                        // The connection closes with no reply, which the client reports as a failure.
                        // An empty 200 would read as "no speech" and fail silently.
                        Err(e) => {
                            warn!("transcription failed: {e}");
                            Ok(())
                        }
                    }
                }
            }
        }
        _ => refuse(&mut stream, &request, 501, "unsupported method; use GET or POST"),
    }
}

impl App {
    fn authorized(&self, header: &str) -> bool {
        let wanted = format!("Bearer {}", self.token.lock().unwrap());
        // Every byte is compared, so the time taken says nothing about how much of a guess was right.
        header.len() == wanted.len() && header.bytes().zip(wanted.bytes()).fold(0, |seen, (a, b)| seen | (a ^ b)) == 0
    }

    fn polish_on(&self) -> bool {
        self.parts.polisher.is_some() && (self.parts.polish_enabled)()
    }

    fn health(&self) -> Value {
        let on = self.polish_on();
        if let Some(polisher) = self.parts.polisher.as_ref().filter(|_| on) {
            let polisher = polisher.clone();
            thread::spawn(move || polisher.preconnect());
        }
        json!({"status": "ok", "postprocess": on})
    }

    fn transcribe(
        &self,
        audio: &Audio,
        postprocess: Option<bool>,
        instructions: &str,
        metrics: &mut Metrics,
    ) -> Result<Value, String> {
        let started = clock::now();
        let raw = self.parts.model.transcribe(audio)?;
        metrics.asr_ms = Some((clock::now() - started) * 1000.0);
        let polish = postprocess.map_or_else(|| self.polish_on(), |wanted| wanted && self.parts.polisher.is_some());
        let polisher = self.parts.polisher.as_ref().filter(|_| polish && !raw.is_empty());
        let Some(polisher) = polisher else {
            return Ok(json!({"text": raw, "raw": raw, "polished": false}));
        };
        let started = clock::now();
        let (done, result) = mpsc::channel();
        let (polisher_now, raw_now, instructions_now) = (polisher.clone(), raw.clone(), instructions.to_owned());
        // Its own thread, so a post-processing request past its timeout never delays the next one.
        thread::spawn(move || {
            let _ = done.send(polisher_now.polish(&raw_now, &instructions_now));
        });
        let text = match result.recv_timeout(Duration::from_secs_f64(self.parts.timeout_s)) {
            Ok(Ok(text)) => Some(text),
            Ok(Err(e)) => {
                warn!("post-processing failed ({e}); returning the raw transcript");
                None
            }
            Err(RecvTimeoutError::Timeout) => {
                let seconds = short_float(self.parts.timeout_s);
                warn!("post-processing failed (no reply within {seconds} s); returning the raw transcript");
                None
            }
            Err(RecvTimeoutError::Disconnected) => {
                warn!("post-processing crashed; returning the raw transcript");
                None
            }
        };
        metrics.polish_ms = Some((clock::now() - started) * 1000.0);
        let polished = text.is_some();
        let text = text.filter(|text| !text.is_empty()).unwrap_or_else(|| raw.clone());
        Ok(json!({"text": text, "raw": raw, "polished": polished}))
    }
}

fn parse_transcription(content_type: &str, body: &[u8]) -> Result<(Audio, Option<bool>, String), BadRequest> {
    let fields = parse_form(content_type, body)?;
    let field = |name: &str| fields.iter().rev().find(|(key, _)| key == name).map(|(_, value)| *value);
    let file = field("file").ok_or_else(|| BadRequest("the form has no field named file".to_owned()))?;
    let audio = parse_wav(file)?;
    let postprocess = parse_flag(field("postprocess"))?;
    let instructions = parse_instructions(field("instructions"))?;
    Ok((audio, postprocess, instructions))
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let first = *needle.first()?;
    let mut at = from;
    while at + needle.len() <= haystack.len() {
        let offset = haystack[at..=haystack.len() - needle.len()].iter().position(|byte| *byte == first)?;
        at += offset;
        if &haystack[at..at + needle.len()] == needle {
            return Some(at);
        }
        at += 1;
    }
    None
}

/// A header parameter such as `name="file"`, unquoted.
fn parameter(header: &str, wanted: &str) -> Option<String> {
    let mut rest = header.split_once(';')?.1;
    loop {
        rest = rest.trim_start_matches([' ', '\t', ';']);
        let (name, value) = rest.split_once('=')?;
        let (value, after) = if let Some(quoted) = value.strip_prefix('"') {
            let mut text = String::new();
            let mut chars = quoted.char_indices();
            let mut end = quoted.len();
            while let Some((index, character)) = chars.next() {
                match character {
                    '\\' => text.extend(chars.next().map(|(_, escaped)| escaped)),
                    '"' => {
                        end = index + 1;
                        break;
                    }
                    other => text.push(other),
                }
            }
            (text, &quoted[end..])
        } else {
            let end = value.find(';').unwrap_or(value.len());
            (value[..end].trim().to_owned(), &value[end..])
        };
        if name.trim().eq_ignore_ascii_case(wanted) {
            return Some(value);
        }
        rest = after;
    }
}

pub fn parse_form<'a>(content_type: &str, body: &'a [u8]) -> Result<Vec<(String, &'a [u8])>, BadRequest> {
    let wrong = || BadRequest(format!("send multipart/form-data with the audio in a field named file, {EXPECTED_AUDIO}"));
    let kind = content_type.split(';').next().unwrap_or("").trim();
    if !kind.eq_ignore_ascii_case("multipart/form-data") {
        return Err(wrong());
    }
    let boundary = parameter(content_type, "boundary").filter(|boundary| !boundary.is_empty()).ok_or_else(wrong)?;
    let delimiter = format!("--{boundary}").into_bytes();
    let mut at = find(body, &delimiter, 0).ok_or_else(wrong)? + delimiter.len();
    let closing = format!("\r\n--{boundary}").into_bytes();
    let mut fields = Vec::new();
    loop {
        if body[at..].starts_with(b"--") {
            break;
        }
        let Some(line_end) = find(body, b"\r\n", at) else {
            break;
        };
        let head_start = line_end + 2;
        let Some(head_end) = find(body, b"\r\n\r\n", head_start) else {
            if body[head_start..].starts_with(b"\r\n") {
                at = head_start + 2;
                continue;
            }
            break;
        };
        let head = String::from_utf8_lossy(&body[head_start..head_end]);
        let content = head_end + 4;
        let end = find(body, &closing, content).unwrap_or(body.len());
        let disposition = head.split("\r\n").find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim().eq_ignore_ascii_case("content-disposition").then(|| value.trim().to_owned())
        });
        if let Some(name) = disposition.as_deref().and_then(|value| parameter(value, "name")) {
            fields.push((name, &body[content..end]));
        }
        if end + closing.len() > body.len() {
            break;
        }
        at = end + closing.len();
    }
    Ok(fields)
}

pub fn parse_wav(data: &[u8]) -> Result<Audio, BadRequest> {
    let (width, channels, rate, frames) = read_wav(data).map_err(|e| BadRequest(format!("the file is not {EXPECTED_AUDIO} ({e})")))?;
    if width != 2 {
        return Err(BadRequest(format!("the file has {}-bit samples; send {EXPECTED_AUDIO}", 8 * width)));
    }
    if rate == 0 {
        return Err(BadRequest(format!("the file has a sample rate of {rate}; send {EXPECTED_AUDIO}")));
    }
    let samples: Vec<i16> = frames.chunks_exact(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]])).collect();
    let mono = samples
        .chunks_exact(channels)
        .map(|frame| {
            let total: f64 = frame.iter().map(|sample| f64::from(*sample)).sum();
            (total / channels as f64 / 32768.0) as f32
        })
        .collect();
    Ok(Audio { samples: mono, rate })
}

/// (bytes per sample, channels, rate, sample data) of a RIFF WAVE file.
fn read_wav(data: &[u8]) -> Result<(usize, usize, u32, &[u8]), String> {
    const EXTENSIBLE: u16 = 0xFFFE;
    let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
    if data.len() < 4 || &data[..4] != b"RIFF" {
        return Err("file does not start with RIFF id".to_owned());
    }
    if data.len() < 12 || &data[8..12] != b"WAVE" {
        return Err("not a WAVE file".to_owned());
    }
    let mut format = None;
    let mut at = 12;
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let size = u32_at(at + 4) as usize;
        let content = at + 8;
        if id == b"fmt " {
            if size < 16 || content + 16 > data.len() {
                return Err("bad format chunk".to_owned());
            }
            let (tag, channels, rate, bits) = (u16_at(content), u16_at(content + 2), u32_at(content + 4), u16_at(content + 14));
            let pcm = tag == 1 || (tag == EXTENSIBLE && size >= 26 && content + 26 <= data.len() && u16_at(content + 24) == 1);
            if !pcm {
                return Err(format!("unknown format: {tag}"));
            }
            if channels == 0 {
                return Err("bad # of channels".to_owned());
            }
            if bits == 0 {
                return Err("bad sample width".to_owned());
            }
            format = Some((usize::from(bits).div_ceil(8), usize::from(channels), rate));
        } else if id == b"data" {
            let Some((width, channels, rate)) = format else {
                return Err("data chunk before fmt chunk".to_owned());
            };
            let end = content.saturating_add(size).min(data.len());
            return Ok((width, channels, rate, &data[content..end]));
        }
        at = content.saturating_add(size).saturating_add(size % 2);
    }
    Err("fmt chunk and/or data chunk missing".to_owned())
}

fn bytes_repr(value: &[u8]) -> String {
    let mut shown = String::from("b'");
    for byte in value {
        match byte {
            b'\'' => shown.push_str("\\'"),
            b'\\' => shown.push_str("\\\\"),
            b'\n' => shown.push_str("\\n"),
            b'\r' => shown.push_str("\\r"),
            b'\t' => shown.push_str("\\t"),
            0x20..=0x7E => shown.push(char::from(*byte)),
            other => shown.push_str(&format!("\\x{other:02x}")),
        }
    }
    shown.push('\'');
    shown
}

pub fn parse_flag(value: Option<&[u8]>) -> Result<Option<bool>, BadRequest> {
    match value {
        None => Ok(None),
        Some(b"true") => Ok(Some(true)),
        Some(b"false") => Ok(Some(false)),
        Some(other) => {
            let start = &other[..other.len().min(20)];
            Err(BadRequest(format!("postprocess must be true or false; got {}", bytes_repr(start))))
        }
    }
}

pub fn parse_instructions(value: Option<&[u8]>) -> Result<String, BadRequest> {
    let text = std::str::from_utf8(value.unwrap_or_default()).map_err(|_| BadRequest("instructions must be UTF-8 text".to_owned()))?;
    Ok(text.trim().to_owned())
}

fn base64_url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().fold(0u32, |group, byte| group << 8 | u32::from(*byte)) << (8 * (3 - chunk.len()));
        for index in 0..=chunk.len() {
            text.push(char::from(ALPHABET[(group >> (18 - 6 * index) & 63) as usize]));
        }
    }
    text
}

pub fn read_token(path: &Path) -> io::Result<String> {
    let token = match fs::read_to_string(path) {
        Ok(token) => token.trim().to_owned(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    if !token.is_empty() {
        return Ok(token);
    }
    let mut random = [0u8; 32];
    let status =
        unsafe { BCryptGenRandom(std::ptr::null_mut(), random.as_mut_ptr(), random.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status != 0 {
        return Err(io::Error::other(format!("BCryptGenRandom failed: {status:#x}")));
    }
    let token = base64_url(&random);
    let temporary = temporary_beside(path, ".tmp");
    fs::write(&temporary, &token)?;
    fs::rename(&temporary, path)?;
    info!("created a new token in {}", path.file_name().unwrap_or_default().to_string_lossy());
    Ok(token)
}

/// This PC's IPv4 address on the local network. Only physical adapters count, which leaves out VPN
/// tunnels such as NordLynx; among those, the one with the default route wins.
pub fn lan_ipv4() -> Option<String> {
    let listed = run_lan_query();
    let candidates: Value = match listed.and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string())) {
        Ok(candidates) => candidates,
        Err(e) => {
            log::error!("could not list the network adapters: {e}");
            return None;
        }
    };
    best_address(candidates.as_array()?)
}

pub fn best_address(candidates: &[Value]) -> Option<String> {
    let mut usable: Vec<(&str, Option<i64>)> = candidates
        .iter()
        .filter_map(|candidate| Some((candidate.get("address")?.as_str()?, candidate.get("route_metric").and_then(Value::as_i64))))
        .filter(|(address, _)| !address.starts_with("169.254."))
        .collect();
    usable.sort_by_key(|(_, metric)| (metric.is_none(), metric.unwrap_or(0)));
    usable.first().map(|(address, _)| (*address).to_owned())
}

fn run_lan_query() -> Result<String, String> {
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", LAN_QUERY])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let (done, output) = mpsc::channel();
    thread::spawn(move || {
        let mut text = String::new();
        let _ = done.send(stdout.read_to_string(&mut text).map(|_| text));
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    match output.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(text) => {
            let _ = child.wait();
            text.map_err(|e| e.to_string())
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err("PowerShell took too long".to_owned())
        }
    }
}
