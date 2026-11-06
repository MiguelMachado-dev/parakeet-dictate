//! Parakeet TDT on ONNX Runtime: resample, log-mel features, encoder, then greedy transducer decoding.
//! The graphs and their parameters are the ones onnx_asr ran, so transcripts stay the same.

use std::collections::HashMap;
use std::f64::consts::TAU;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use log::{info, warn};
use ort::environment::Environment;
use ort::ep::{CPU, CUDA, ExecutionProviderDispatch};
use ort::logging::LogLevel;
use ort::session::builder::SessionBuilder;
use ort::session::{OutputSelector, RunOptions, Session};
use ort::value::TensorRef;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex64;
use windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW;
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, SetThreadInformation, THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    THREAD_POWER_THROTTLING_STATE, ThreadPowerThrottling,
};

use crate::clock;
use crate::config::Engine;
use crate::paths;
use crate::recorder::Audio;
use crate::text::has_word_char;
use crate::win::wide_path;

pub const MIN_SECONDS: f64 = 0.1;
const WARMUP_SECONDS: [f64; 2] = [1.0, 4.0];
const TARGET_RATE: u32 = 16_000;
const MAX_TOKENS_PER_STEP: u32 = 10;
const ENCODER_WIDTH: usize = 1024;
const STATE_LAYERS: usize = 2;
const STATE_WIDTH: usize = 640;

const FEATURES: &[u8] = include_bytes!("../assets/onnx/nemo128_conv.onnx");

/// The resampling graph from a microphone rate to the model's 16 kHz. None for rates without one.
fn resampling_graph(rate: u32) -> Option<&'static [u8]> {
    Some(match rate {
        8_000 => include_bytes!("../assets/onnx/resample_8_16.onnx"),
        11_025 => include_bytes!("../assets/onnx/resample_11_16.onnx"),
        22_050 => include_bytes!("../assets/onnx/resample_22_16.onnx"),
        24_000 => include_bytes!("../assets/onnx/resample_24_16.onnx"),
        32_000 => include_bytes!("../assets/onnx/resample_32_16.onnx"),
        44_100 => include_bytes!("../assets/onnx/resample_44_16.onnx"),
        48_000 => include_bytes!("../assets/onnx/resample_48_16.onnx"),
        _ => return None,
    })
}

/// Tag the calling thread high QoS. Untagged, Windows can run it on an efficiency core, where a
/// transcription took two to four times as long (docs/perf/README.md).
pub fn prefer_performance_cores() {
    let state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    unsafe {
        SetThreadInformation(GetCurrentThread(), ThreadPowerThrottling, (&raw const state).cast(), size_of_val(&state) as u32);
    }
}

pub trait Transcribe: Send + Sync {
    fn transcribe(&self, audio: &Audio) -> Result<String, String>;
}

/// What the app runs once a model is loaded. Tests supply their own.
pub trait LoadedModel: Send {
    fn transcribe(&mut self, audio: &Audio) -> Result<String, String>;
}

/// The one loaded model, shared by local dictation and the server for Mac. It runs one transcription
/// at a time: a caller that arrives during another's waits for it.
#[derive(Default)]
pub struct SharedModel {
    transcriber: Mutex<Option<Box<dyn LoadedModel>>>,
    ready: AtomicBool,
}

impl SharedModel {
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub fn load(&self, transcriber: Box<dyn LoadedModel>) {
        *self.transcriber.lock().unwrap() = Some(transcriber);
        self.ready.store(true, Ordering::Release);
    }
}

impl Transcribe for SharedModel {
    fn transcribe(&self, audio: &Audio) -> Result<String, String> {
        let mut transcriber = self.transcriber.lock().unwrap();
        transcriber.as_mut().ok_or("the model is not loaded")?.transcribe(audio)
    }
}

impl LoadedModel for Transcriber {
    fn transcribe(&mut self, audio: &Audio) -> Result<String, String> {
        Self::transcribe(self, audio)
    }
}

pub struct Transcriber {
    engine: Engine,
    encoder: Session,
    decoder: Session,
    features: Session,
    resamplers: HashMap<u32, Session>,
    vocabulary: Vec<String>,
    blank: usize,
    decoder_options: RunOptions<ort::session::HasSelectedOutputs>,
}

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn runtime() -> Result<&'static Environment, String> {
    static RUNTIME: OnceLock<Result<Environment, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            let folder = paths::runtime();
            let library = folder.join("onnxruntime.dll");
            if !library.is_file() {
                return Err(format!("no ONNX Runtime in {}; run scripts/fetch_assets.ps1", folder.display()));
            }
            // The CUDA provider finds its NVIDIA libraries here.
            unsafe { SetDllDirectoryW(wide_path(&folder).as_ptr()) };
            ort::init_from(&library).map_err(error)?.with_telemetry(false).build().map_err(error)
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// One thread: features take milliseconds, and a thread per core would only sit idle for this session.
fn open_features(engine: Engine) -> ort::Result<Session> {
    session_builder(engine)?.with_intra_threads(1)?.commit_from_memory(FEATURES)
}

fn session_builder(engine: Engine) -> ort::Result<SessionBuilder> {
    let environment = runtime().map_err(ort::Error::new)?;
    let builder = Session::builder(environment)?.with_log_level(LogLevel::Error)?;
    Ok(match engine {
        Engine::Cuda => {
            // The GPU runs the graphs and the CPU only does shape arithmetic, so the default pool of a thread per
            // core in every session would only spin after each run.
            let providers: [ExecutionProviderDispatch; 2] = [CUDA::default().build().error_on_failure(), CPU::default().build()];
            builder.with_execution_providers(providers)?.with_intra_threads(1)?
        }
        // The encoder needs the default pool of a thread per core, but every session's pool spins after each
        // run, through the other sessions' runs and for seconds after a transcription ends.
        Engine::Cpu => builder.with_config_entry("session.force_spinning_stop", "1")?,
    })
}

/// (token text by id, the blank's id) from vocab.txt, whose lines are "token id".
fn read_vocabulary(path: &Path) -> Result<(Vec<String>, usize), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut tokens = Vec::new();
    for line in text.lines() {
        let (token, id) = line.rsplit_once(' ').ok_or("vocab.txt: a line has no id")?;
        let id: usize = id.parse().map_err(|_| "vocab.txt: a line has no id")?;
        if tokens.len() <= id {
            tokens.resize(id + 1, String::new());
        }
        tokens[id] = token.replace('\u{2581}', " ");
    }
    let blank = tokens.iter().position(|token| token == "<blk>").ok_or("vocab.txt has no <blk>")?;
    Ok((tokens, blank))
}

/// Keep a space only before a word character, and drop the first character when it is a space.
pub fn join_tokens(tokens: &[&str]) -> String {
    let text: Vec<char> = tokens.concat().chars().collect();
    let mut kept = String::with_capacity(text.len());
    for (index, character) in text.iter().enumerate() {
        if !character.is_whitespace() {
            kept.push(*character);
        } else if index > 0 && text.get(index + 1).is_some_and(|next| crate::text::is_word_char(*next)) {
            kept.push(' ');
        }
    }
    kept
}

/// The next step of the TDT decoding loop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub frame: usize,
    pub emitted: u32,
}

impl Cursor {
    /// Move past a decoder step that read `token` and predicted a jump of `step` frames.
    pub fn advance(&mut self, token: usize, blank: usize, step: usize) {
        if token != blank {
            self.emitted += 1;
        }
        if step > 0 {
            self.frame += step;
            self.emitted = 0;
        } else if token == blank || self.emitted == MAX_TOKENS_PER_STEP {
            self.frame += 1;
            self.emitted = 0;
        }
    }
}

fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}

impl Transcriber {
    pub fn load(model_dir: &Path, engine: Engine) -> Result<Self, String> {
        if !model_dir.join("config.json").exists() {
            return Err(format!("no model in {}; run scripts/fetch_assets.ps1", model_dir.display()));
        }
        runtime()?;
        if engine == Engine::Cuda {
            match Self::open(model_dir, Engine::Cuda) {
                Ok(transcriber) => return Ok(transcriber),
                Err(e) => warn!("CUDA load failed; falling back to CPU int8: {e}"),
            }
        }
        Self::open(model_dir, Engine::Cpu)
    }

    fn open(model_dir: &Path, engine: Engine) -> Result<Self, String> {
        let started = clock::now();
        let suffix = if engine == Engine::Cpu { ".int8" } else { "" };
        let graph = |name: &str| model_dir.join(format!("{name}{suffix}.onnx"));
        let open_file = |path: &Path| session_builder(engine).and_then(|mut builder| builder.commit_from_file(path)).map_err(error);
        let encoder = open_file(&graph("encoder-model"))?;
        let decoder = open_file(&graph("decoder_joint-model"))?;
        let features = open_features(engine).map_err(error)?;
        let (vocabulary, blank) = read_vocabulary(&model_dir.join("vocab.txt"))?;
        let selection = OutputSelector::no_default().with("outputs").with("output_states_1").with("output_states_2");
        let decoder_options = RunOptions::new().map_err(error)?.with_outputs(selection);
        let transcriber =
            Self { engine, encoder, decoder, features, resamplers: HashMap::new(), vocabulary, blank, decoder_options };
        let folder = model_dir.file_name().unwrap_or_default().to_string_lossy();
        info!("loaded {folder} on {} in {:.1} s", transcriber.label(), clock::now() - started);
        Ok(transcriber)
    }

    pub fn engine(&self) -> Engine {
        self.engine
    }

    pub fn label(&self) -> &'static str {
        match self.engine {
            Engine::Cuda => "CUDA",
            Engine::Cpu => "CPU int8",
        }
    }

    pub fn transcribe(&mut self, audio: &Audio) -> Result<String, String> {
        if audio.seconds() < MIN_SECONDS {
            return Ok(String::new());
        }
        prefer_performance_cores();
        let resampled;
        let samples: &[f32] = if audio.rate == TARGET_RATE {
            &audio.samples
        } else if let Some(graph) = resampling_graph(audio.rate) {
            resampled = self.resample_with_graph(&audio.samples, audio.rate, graph)?;
            &resampled
        } else {
            resampled = resample(&audio.samples, audio.rate, TARGET_RATE);
            &resampled
        };
        let (features, frames) = self.extract_features(samples)?;
        let (encoded, encoded_frames) = self.encode(features, frames)?;
        let tokens = self.decode(&encoded, encoded_frames).map_err(error)?;
        let parts: Vec<&str> = tokens.iter().map(|token| self.vocabulary[*token].as_str()).collect();
        let joined = join_tokens(&parts);
        let text = joined.trim();
        Ok(if has_word_char(text) { text.to_owned() } else { String::new() })
    }

    pub fn warm_up(&mut self, rate: u32) -> Result<(), String> {
        let noise = warm_up_noise((WARMUP_SECONDS[1] * f64::from(rate)) as usize);
        for seconds in WARMUP_SECONDS {
            let started = clock::now();
            let length = (seconds * f64::from(rate)) as usize;
            self.transcribe(&Audio { samples: noise[..length].to_vec(), rate })?;
            info!("warm-up {seconds:.1} s @ {rate} Hz: {:.0} ms", (clock::now() - started) * 1000.0);
        }
        Ok(())
    }

    fn resample_with_graph(&mut self, samples: &[f32], rate: u32, graph: &[u8]) -> Result<Vec<f32>, String> {
        // One session for each rate that arrives: a session costs about 70 MB of GPU memory and a
        // microphone records at one rate.
        if !self.resamplers.contains_key(&rate) {
            let session = session_builder(self.engine).and_then(|mut builder| builder.commit_from_memory(graph)).map_err(error)?;
            self.resamplers.insert(rate, session);
        }
        let session = self.resamplers.get_mut(&rate).expect("just opened");
        let lengths = [samples.len() as i64];
        let outputs = session
            .run(ort::inputs![
                "waveforms" => TensorRef::from_array_view(([1usize, samples.len()], samples)).map_err(error)?,
                "waveforms_lens" => TensorRef::from_array_view(([1usize], &lengths[..])).map_err(error)?,
            ])
            .map_err(error)?;
        let (_, resampled) = outputs["resampled"].try_extract_tensor::<f32>().map_err(error)?;
        let (_, kept) = outputs["resampled_lens"].try_extract_tensor::<i64>().map_err(error)?;
        Ok(resampled[..(kept[0] as usize).min(resampled.len())].to_vec())
    }

    /// The features as (values of shape 1 x 128 x T, their valid T).
    fn extract_features(&mut self, samples: &[f32]) -> Result<(ort::value::DynValue, i64), String> {
        let lengths = [samples.len() as i64];
        let mut outputs = self
            .features
            .run(ort::inputs![
                "waveforms" => TensorRef::from_array_view(([1usize, samples.len()], samples)).map_err(error)?,
                "waveforms_lens" => TensorRef::from_array_view(([1usize], &lengths[..])).map_err(error)?,
            ])
            .map_err(error)?;
        let (_, kept) = outputs["features_lens"].try_extract_tensor::<i64>().map_err(error)?;
        let frames = kept[0];
        let features = outputs.remove("features").ok_or("the feature graph gave no features")?;
        Ok((features, frames))
    }

    /// The encoder's output as frames of ENCODER_WIDTH values, and how many of them are valid.
    fn encode(&mut self, features: ort::value::DynValue, frames: i64) -> Result<(Vec<f32>, usize), String> {
        let lengths = [frames];
        let outputs = self
            .encoder
            .run(ort::inputs![
                "audio_signal" => features,
                "length" => TensorRef::from_array_view(([1usize], &lengths[..])).map_err(error)?,
            ])
            .map_err(error)?;
        let (shape, encoded) = outputs["outputs"].try_extract_tensor::<f32>().map_err(error)?;
        let (_, kept) = outputs["encoded_lengths"].try_extract_tensor::<i64>().map_err(error)?;
        let (width, total) = (shape[1] as usize, shape[2] as usize);
        let valid = (kept[0] as usize).min(total);
        let mut transposed = vec![0f32; width * valid];
        for frame in 0..valid {
            for channel in 0..width {
                transposed[frame * width + channel] = encoded[channel * total + frame];
            }
        }
        Ok((transposed, valid))
    }

    fn decode(&mut self, encoded: &[f32], frames: usize) -> ort::Result<Vec<usize>> {
        let (blank, vocabulary_size) = (self.blank, self.vocabulary.len());
        let mut state1 = vec![0f32; STATE_LAYERS * STATE_WIDTH];
        let mut state2 = vec![0f32; STATE_LAYERS * STATE_WIDTH];
        let state_shape = [STATE_LAYERS, 1, STATE_WIDTH];
        let mut tokens: Vec<usize> = Vec::new();
        let mut cursor = Cursor::default();
        let length = [1i32];
        while cursor.frame < frames {
            let frame = &encoded[cursor.frame * ENCODER_WIDTH..(cursor.frame + 1) * ENCODER_WIDTH];
            let previous = [tokens.last().copied().unwrap_or(blank) as i32];
            let outputs = self.decoder.run_with_options(
                ort::inputs![
                    "encoder_outputs" => TensorRef::from_array_view(([1usize, ENCODER_WIDTH, 1], frame))?,
                    "targets" => TensorRef::from_array_view(([1usize, 1], &previous[..]))?,
                    "target_length" => TensorRef::from_array_view(([1usize], &length[..]))?,
                    "input_states_1" => TensorRef::from_array_view((state_shape, &state1[..]))?,
                    "input_states_2" => TensorRef::from_array_view((state_shape, &state2[..]))?,
                ],
                &self.decoder_options,
            )?;
            let (_, output) = outputs["outputs"].try_extract_tensor::<f32>()?;
            let token = argmax(&output[..vocabulary_size]);
            let step = argmax(&output[vocabulary_size..]);
            if token != blank {
                state1.copy_from_slice(outputs["output_states_1"].try_extract_tensor::<f32>()?.1);
                state2.copy_from_slice(outputs["output_states_2"].try_extract_tensor::<f32>()?.1);
                tokens.push(token);
            }
            cursor.advance(token, blank, step);
        }
        Ok(tokens)
    }
}

/// Gaussian noise of deviation 0.01 from a fixed seed, the audio that warms up the model's graphs.
fn warm_up_noise(length: usize) -> Vec<f32> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut uniform = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..length)
        .map(|_| {
            let radius = (-2.0 * (1.0 - uniform()).ln()).sqrt();
            (0.01 * radius * (TAU * uniform()).cos()) as f32
        })
        .collect()
}

/// Resample by cropping or padding the spectrum, for rates without a graph.
pub fn resample(samples: &[f32], rate: u32, target: u32) -> Vec<f32> {
    let n_in = samples.len();
    let n_out = (n_in as f64 * f64::from(target) / f64::from(rate)).round_ties_even() as usize;
    if n_in == 0 || n_out == 0 {
        return Vec::new();
    }
    let mut planner = FftPlanner::<f64>::new();
    let mut spectrum: Vec<Complex64> = samples.iter().map(|sample| Complex64::new(f64::from(*sample), 0.0)).collect();
    planner.plan_fft_forward(n_in).process(&mut spectrum);
    let kept = (n_in / 2 + 1).min(n_out / 2 + 1);
    let mut output = vec![Complex64::new(0.0, 0.0); n_out];
    output[..kept].copy_from_slice(&spectrum[..kept]);
    output[0].im = 0.0;
    if n_out % 2 == 0 {
        output[n_out / 2].im = 0.0;
    }
    for bin in 1..n_out.div_ceil(2) {
        output[n_out - bin] = output[bin].conj();
    }
    planner.plan_fft_inverse(n_out).process(&mut output);
    // rustfft leaves the inverse unscaled, so n_out / n_in over n_out is 1 / n_in.
    output.iter().map(|value| (value.re / n_in as f64) as f32).collect()
}
