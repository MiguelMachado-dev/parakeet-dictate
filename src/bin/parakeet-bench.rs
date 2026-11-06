//! The child process of scripts/bench_resources.py: runs the same work as the harness's model and overlay
//! children, through the app's own code, on audio the harness exported. The harness reads this process's
//! counters from outside, each time this process asks for a snapshot on stdout.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use parakeet_dictate::app_rules::AppRule;
use parakeet_dictate::clock;
use parakeet_dictate::config::{Config, Engine, PostprocessConfig};
use parakeet_dictate::cues::PlayCue;
use parakeet_dictate::dictation::{Dictation, DictationParts};
use parakeet_dictate::meter::LevelMeter;
use parakeet_dictate::overlay::frame::{FPS, Phase};
use parakeet_dictate::overlay::scene::{Command, Placement, Scene, place};
use parakeet_dictate::overlay::{Style, premultiplied_bgra};
use parakeet_dictate::polish::{Headers, Polish, Reply, Transport, TransportError, create_polisher};
use parakeet_dictate::recorder::{Audio, Record};
use parakeet_dictate::reducer::{Cue, Event};
use parakeet_dictate::transcriber::{SharedModel, Transcribe, Transcriber};
use serde_json::{Value, json};
use windows_sys::Win32::System::Threading::GetCurrentProcessorNumber;

const RATE: u32 = 44_100;
const CLIPS: [&str; 6] = ["short", "en", "pt", "mid", "long", "xlong"];
const SAME_LENGTH_CLIPS: [&str; 3] = ["short", "en", "mid"];
const OVERLAY_CLIPS: [&str; 3] = ["short", "en", "mid"];
const DICTATION_CLIPS: [&str; 4] = ["short", "en", "pt", "mid"];

/// Ask the harness to read this process's counters, and wait until it has.
fn snapshot(name: &str, gpu: bool) {
    println!("snapshot {name} {}", if gpu { "gpu" } else { "plain" });
    let _ = io::stdout().flush();
    let mut answer = String::new();
    let _ = io::stdin().lock().read_line(&mut answer);
}

fn core() -> u32 {
    unsafe { GetCurrentProcessorNumber() }
}

fn option(arguments: &[String], name: &str) -> String {
    let at = arguments.iter().position(|argument| argument == name).unwrap_or_else(|| panic!("missing {name}"));
    arguments[at + 1].clone()
}

/// The audio file the harness exported: entries of (name, f32 samples) in the order this process uses them.
///
/// Read one entry at a time, so the file's size stays out of this process's memory counters.
struct Takes(BufReader<File>);

impl Takes {
    fn open(path: &Path) -> Self {
        Self(BufReader::new(File::open(path).expect("the exported audio")))
    }

    fn bytes(&mut self, length: usize) -> Vec<u8> {
        let mut bytes = vec![0; length];
        self.0.read_exact(&mut bytes).expect("the audio file is complete");
        bytes
    }

    fn word(&mut self) -> usize {
        u32::from_le_bytes(self.bytes(4).try_into().expect("four bytes")) as usize
    }

    /// The next entry, which must be `expected`.
    fn next(&mut self, expected: &str) -> Vec<f32> {
        let length = self.word();
        let name = String::from_utf8(self.bytes(length)).expect("a name");
        assert_eq!(name, expected, "the audio file is out of step");
        let samples = self.word();
        self.bytes(samples * 4).chunks_exact(4).map(|sample| f32::from_le_bytes(sample.try_into().expect("four bytes"))).collect()
    }
}

struct Run {
    model: Arc<SharedModel>,
    sets: serde_json::Map<String, Value>,
}

impl Run {
    fn transcribe(&mut self, set: &str, name: &str, samples: Vec<f32>) {
        let seconds = samples.len() as f64 / f64::from(RATE);
        let audio = Audio { samples, rate: RATE };
        let before = core();
        let started = clock::now();
        let result = self.model.transcribe(&audio);
        let ms = (clock::now() - started) * 1000.0;
        let mut row = json!({"clip": name, "seconds": seconds, "ms": ms, "cores": [before, core()]});
        match result {
            Ok(text) => row["text"] = json!(text),
            Err(e) => row["error"] = json!(e),
        }
        self.sets.entry(set.to_owned()).or_insert_with(|| json!([])).as_array_mut().expect("rows").push(row);
    }
}

struct ClipRecorder(Arc<Mutex<Vec<f32>>>);

impl Record for ClipRecorder {
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn stop(&mut self) -> Result<Audio, String> {
        Ok(Audio { samples: self.0.lock().unwrap().clone(), rate: RATE })
    }

    fn discard(&mut self) {}

    fn device_name(&self) -> String {
        "fixture".to_owned()
    }
}

/// Records the cores Dictation's transcription thread ran on, before and after each transcription.
struct CoreRecorder {
    model: Arc<SharedModel>,
    cores: Arc<Mutex<[u32; 2]>>,
}

impl Transcribe for CoreRecorder {
    fn transcribe(&self, audio: &Audio) -> Result<String, String> {
        let before = core();
        let result = self.model.transcribe(audio);
        *self.cores.lock().unwrap() = [before, core()];
        result
    }
}

struct Silent;

impl PlayCue for Silent {
    fn play(&self, _: Cue) {}
}

/// Hold-to-talk dictations through Dictation's controller thread, timed from key-up to the paste call.
fn dictate(
    model: &Arc<SharedModel>,
    config: Config,
    polisher: Option<Arc<dyn Polish>>,
    foreground: Option<&'static str>,
    clips: Vec<Vec<f32>>,
) -> Vec<Value> {
    let (pasted, paste_seen) = mpsc::channel();
    let clip = Arc::new(Mutex::new(Vec::new()));
    let cores = Arc::new(Mutex::new([0, 0]));
    let polishing = polisher.is_some();
    let mut parts = DictationParts::new(
        config,
        Box::new(ClipRecorder(clip.clone())),
        Arc::new(CoreRecorder { model: model.clone(), cores: cores.clone() }),
        Arc::new(Silent),
        Box::new(|_| {}),
        polisher,
        Arc::new(move || polishing),
    );
    parts.paste = Box::new(move |text| {
        let _ = pasted.send((clock::now(), text.to_owned()));
        Ok(())
    });
    parts.foreground_app = Box::new(move || foreground.map(str::to_owned));
    let dictation = Dictation::new(parts);
    let mut rows = Vec::new();
    for samples in clips {
        let seconds = samples.len() as f64 / f64::from(RATE);
        *clip.lock().unwrap() = samples;
        dictation.post(Event::HotkeyDown(clock::now() - 2.0));
        let released = clock::now();
        dictation.post(Event::HotkeyUp(released));
        let row = match paste_seen.recv_timeout(Duration::from_secs(30)) {
            Ok((at, text)) => {
                let cores = *cores.lock().unwrap();
                json!({"seconds": seconds, "ms": (at - released) * 1000.0, "text": text, "cores": cores})
            }
            Err(_) => json!({"seconds": seconds, "error": "no paste within 30 s"}),
        };
        rows.push(row);
    }
    dictation.stop();
    rows
}

/// Answers the GLM client like Z.ai would, at once, so the timing is the app's own work.
struct MockProvider {
    rule: &'static str,
    counts: Arc<Mutex<[u64; 3]>>,
}

impl Transport for MockProvider {
    fn options(&self, _: &str, _: &Headers) -> Result<Reply, TransportError> {
        self.counts.lock().unwrap()[2] += 1;
        Ok(Reply { status: 204, body: Vec::new() })
    }

    fn post(&self, _: &str, _: &Headers, body: &[u8]) -> Result<Reply, TransportError> {
        let body: Value = serde_json::from_slice(body).expect("a JSON request");
        {
            let mut counts = self.counts.lock().unwrap();
            counts[0] += 1;
            counts[1] += u64::from(body["messages"][0]["content"].as_str().unwrap_or("").contains(self.rule));
        }
        let user = body["messages"][1]["content"].as_str().unwrap_or("");
        let raw = user.trim_start_matches("<transcript>").trim_end_matches("</transcript>");
        let reply = json!({
            "choices": [{"message": {"content": format!("[polished] {raw}")}, "finish_reason": "stop"}],
            "usage": {"completion_tokens_details": {"reasoning_tokens": 0}},
        });
        Ok(Reply { status: 200, body: serde_json::to_vec(&reply).expect("a reply serializes") })
    }
}

/// The overlay thread's per-frame work at 60 fps, without a window: scene, bars renderer, premultiply.
fn overlay_loop_in_thread() -> (mpsc::Sender<()>, thread::JoinHandle<u64>) {
    let (stop, stopping) = mpsc::channel::<()>();
    let drawing = thread::Builder::new()
        .name("overlay".to_owned())
        .spawn(move || {
            let mut scene = Scene::default();
            let start = clock::now();
            let placement = Placement { x: 1080, y: 1272, width: 400, height: 120, scale: 1.0 };
            scene.apply(Command::Show { phase: Phase::Transcribing, hands_free: false }, start - 1.0, placement);
            let levels: Vec<f32> = (0..96).map(|i| i as f32 / 95.0).collect();
            let spectrum = [0.4f32; 24];
            let mut canvas = vec![0u8; 400 * 120 * 4];
            let mut pixels = vec![0u8; 400 * 120 * 4];
            let mut due = start;
            let mut frames = 0;
            loop {
                let shown = scene.frame(clock::now(), &levels, &spectrum).expect("the scene stays on screen");
                Style::Bars.render(&shown.0, &mut canvas);
                premultiplied_bgra(&canvas, &mut pixels);
                frames += 1;
                due += 1.0 / FPS;
                let wait = Duration::from_secs_f64((due - clock::now()).max(0.0));
                if !matches!(stopping.recv_timeout(wait), Err(RecvTimeoutError::Timeout)) {
                    return frames;
                }
            }
        })
        .expect("a thread starts");
    (stop, drawing)
}

fn run_model(arguments: &[String]) -> Value {
    let model_dir = PathBuf::from(option(arguments, "--model-dir"));
    let engine = if option(arguments, "--provider") == "cpu" { Engine::Cpu } else { Engine::Cuda };
    let reps: usize = option(arguments, "--reps").parse().expect("a count");
    let mut takes = Takes::open(Path::new(&option(arguments, "--audio")));
    takes.next("speech");
    // The Python child holds these source clips for its whole run, so this one does too.
    let held = CLIPS.map(|name| takes.next(name));

    snapshot("python", true);
    snapshot("imported", true);
    let exe = std::env::current_exe().expect("this program");
    let mut result = json!({
        "kind": "model",
        "provider": engine.name(),
        "source": exe.display().to_string(),
        "import_s": 0.0,
    });

    let started = clock::now();
    let mut transcriber = Transcriber::load(&model_dir, engine).expect("the model loads");
    result["load_s"] = json!(clock::now() - started);
    result["label"] = json!(transcriber.label());
    snapshot("loaded", true);
    let started = clock::now();
    transcriber.warm_up(RATE).expect("the warm-up runs");
    result["warm_up_s"] = json!(clock::now() - started);
    snapshot("warm", true);
    let model = Arc::new(SharedModel::default());
    model.load(Box::new(transcriber));

    let mut run = Run { model: model.clone(), sets: serde_json::Map::new() };
    for _ in 0..reps {
        for name in CLIPS {
            let samples = takes.next(name);
            run.transcribe("new_length", name, samples);
        }
    }
    snapshot("after_new_length", true);

    for name in SAME_LENGTH_CLIPS {
        let samples = takes.next(name);
        for _ in 0..reps {
            run.transcribe("same_length", name, samples.clone());
        }
    }
    snapshot("after_same_length", false);

    let (stop_drawing, drawing) = overlay_loop_in_thread();
    for _ in 0..reps.saturating_sub(1).max(1) {
        for name in OVERLAY_CLIPS {
            let samples = takes.next(name);
            run.transcribe("overlay_busy", name, samples);
        }
    }
    drop(stop_drawing);
    result["overlay_busy_frames"] = json!(drawing.join().expect("the drawing thread ends"));
    snapshot("after_overlay_busy", false);

    let plain = Config { model_dir: model_dir.clone(), provider: engine, ..Config::default() };
    let mut clips = Vec::new();
    for _ in 0..reps {
        for name in DICTATION_CLIPS {
            clips.push(takes.next(name));
        }
    }
    run.sets.insert("dictation".to_owned(), Value::Array(dictate(&model, plain, None, None, clips)));
    snapshot("after_dictation", true);

    let rule = "Translate the transcript to English.";
    let counts = Arc::new(Mutex::new([0u64; 3]));
    let postprocess = PostprocessConfig {
        endpoint: "https://mock.invalid/api/coding/paas/v4/chat/completions".to_owned(),
        ..PostprocessConfig::default()
    };
    let polishing = Config {
        model_dir,
        provider: engine,
        postprocess: postprocess.clone(),
        app_rules: vec![AppRule::new("English", rule, &["Slack.exe"])],
        ..Config::default()
    };
    let transport = Box::new(MockProvider { rule, counts: counts.clone() });
    let polisher: Arc<dyn Polish> = Arc::new(create_polisher(&postprocess, "mock-key", Some(transport)));
    let pair = [takes.next("short"), takes.next("en")];
    let clips: Vec<Vec<f32>> = (0..3).flat_map(|_| pair.iter().cloned()).collect();
    let rows = dictate(&model, polishing, Some(polisher), Some("Slack.exe"), clips);
    run.sets.insert("dictation_polish_mock".to_owned(), Value::Array(rows));
    let [posts, with_rule, preconnects] = *counts.lock().unwrap();
    result["mock_provider"] = json!({"posts": posts, "posts_with_rule": with_rule, "preconnects": preconnects});

    snapshot("idle", false);
    thread::sleep(Duration::from_secs(5));
    snapshot("idle_5s", false);
    thread::sleep(Duration::from_secs(25));
    snapshot("idle_30s", true);
    std::hint::black_box(&held);
    result["sets"] = Value::Object(run.sets);
    result
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    let position = (sorted.len() - 1) as f64 * q;
    let low = position as usize;
    let high = (low + 1).min(sorted.len() - 1);
    sorted[low] + (sorted[high] - sorted[low]) * (position - low as f64)
}

fn spread(mut times: Vec<f64>) -> (f64, f64, f64) {
    times.sort_by(f64::total_cmp);
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    (percentile(&times, 0.5), percentile(&times, 0.95), mean)
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3))
}

fn run_overlay(arguments: &[String]) -> Value {
    let repeats: usize = option(arguments, "--repeats").parse().expect("a count");
    let speech = Takes::open(Path::new(&option(arguments, "--audio"))).next("speech");
    let dump = arguments.iter().position(|argument| argument == "--dump").map(|at| PathBuf::from(&arguments[at + 1]));
    let show = |phase, hands_free| Command::Show { phase, hands_free };
    let finish = |beat| Command::Finish { beat: Some(beat) };
    let script = [
        (0.0, show(Phase::Recording, false)),
        (3.0, show(Phase::Transcribing, false)),
        (3.3, show(Phase::Polishing, false)),
        (4.4, finish(Phase::Done)),
        (6.0, show(Phase::Recording, true)),
        (8.0, finish(Phase::Cancelled)),
        (9.5, show(Phase::Recording, false)),
        (10.5, show(Phase::Transcribing, false)),
        (10.9, finish(Phase::Failed)),
        (12.0, show(Phase::Recording, false)),
        (12.1, Command::Vanish),
    ];
    let end_s = 12.5;
    let exe = std::env::current_exe().expect("this program");
    let mut render = serde_json::Map::new();
    let mut hashes = serde_json::Map::new();
    let hop = 441;
    for (style, name) in [(Style::Bars, "bars"), (Style::Fluid, "fluid")] {
        for scale in [1.0f64, 1.5] {
            let placement = place((0, 0, 2560, 1400), (96.0 * scale).round() as u32);
            let bytes = (placement.width * placement.height * 4) as usize;
            let (mut canvas, mut pixels) = (vec![0u8; bytes], vec![0u8; bytes]);
            let mut times = Vec::new();
            let mut digest = FNV_OFFSET;
            let key = if scale == 1.0 { format!("{name}@1") } else { format!("{name}@{scale}") };
            let mut dumped = dump.as_ref().map(|path| {
                io::BufWriter::new(fs::File::create(path.join(format!("{key}.raw"))).expect("the dump folder"))
            });
            for repeat in 0..repeats {
                let meter = LevelMeter::new(RATE);
                let mut scene = Scene::default();
                let mut fed = 0;
                let mut commands = script.iter().peekable();
                for i in 0..(end_s * FPS) as usize {
                    let now = i as f64 / FPS;
                    while let Some((_, command)) = commands.next_if(|(at, _)| *at <= now) {
                        scene.apply(*command, now, placement);
                    }
                    let until = speech.len().min((now * f64::from(RATE)).round_ties_even() as usize);
                    while fed < until {
                        meter.feed(&speech[fed..(fed + hop).min(speech.len())]);
                        fed += hop;
                    }
                    let (levels, spectrum) = meter.snapshot();
                    let started = clock::now();
                    let Some((frame, _)) = scene.frame(now, &levels, &spectrum) else {
                        continue;
                    };
                    style.render(&frame, &mut canvas);
                    premultiplied_bgra(&canvas, &mut pixels);
                    times.push((clock::now() - started) * 1000.0);
                    if repeat == 0 {
                        digest = fnv(digest, &pixels);
                        if let Some(file) = dumped.as_mut() {
                            file.write_all(&pixels).expect("the dump is written");
                        }
                    }
                }
            }
            let (p50, p95, mean) = spread(times.clone());
            render.insert(key.clone(), json!({"frames": times.len(), "p50_ms": p50, "p95_ms": p95, "mean_ms": mean}));
            hashes.insert(key, json!(format!("{digest:016x}")));
        }
    }

    let meter = LevelMeter::new(RATE);
    let mut calls = Vec::new();
    for _ in 0..repeats {
        meter.reset();
        for block in speech.chunks(hop) {
            let started = clock::now();
            meter.feed(block);
            calls.push((clock::now() - started) * 1e6);
        }
    }
    let (levels, spectrum) = meter.snapshot();
    let mut digest = FNV_OFFSET;
    for value in levels.iter().chain(&spectrum) {
        digest = fnv(digest, &value.to_le_bytes());
    }
    let (p50, p95, _) = spread(calls.clone());
    if let Some(path) = &dump {
        let values: Vec<u8> = levels.iter().chain(&spectrum).flat_map(|value| value.to_le_bytes()).collect();
        fs::write(path.join("meter.raw"), values).expect("the dump is written");
    }
    snapshot("threads", false);
    json!({
        "kind": "overlay",
        "source": exe.display().to_string(),
        "render": render,
        "hashes": hashes,
        "meter": {"calls": calls.len(), "block": hop, "p50_us": p50, "p95_us": p95, "hash": format!("{digest:016x}")},
    })
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.first().map(String::as_str) {
        Some("model") => run_model(&arguments),
        Some("overlay") => run_overlay(&arguments),
        _ => {
            eprintln!("usage: parakeet-bench model|overlay ...");
            std::process::exit(2);
        }
    };
    let text = serde_json::to_string(&result).expect("a result serializes");
    fs::write(option(&arguments, "--json-out"), text).expect("the result is written");
    println!("done");
    let _ = io::stdout().flush();
}
