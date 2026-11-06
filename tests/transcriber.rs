//! Port of tests/test_transcriber.py, plus what replaced onnx_asr in Rust. Whatever needs the real model
//! or an ONNX session is covered by the benchmark instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use parakeet_dictate::recorder::Audio;
use parakeet_dictate::transcriber::{Cursor, LoadedModel, MIN_SECONDS, SharedModel, Transcribe, join_tokens, prefer_performance_cores};
use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadInformation, THREAD_POWER_THROTTLING_STATE, ThreadPowerThrottling};

/// (ControlMask, StateMask) of a new thread's power throttling after it runs `transcribe`.
fn power_throttling_after(transcribe: impl FnOnce() + Send + 'static) -> (u32, u32) {
    thread::spawn(move || {
        transcribe();
        let mut state = THREAD_POWER_THROTTLING_STATE { Version: 1, ControlMask: 0, StateMask: 0 };
        let size = size_of_val(&state) as u32;
        let ok = unsafe { GetThreadInformation(GetCurrentThread(), ThreadPowerThrottling, (&raw mut state).cast(), size) };
        assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
        (state.ControlMask, state.StateMask)
    })
    .join()
    .unwrap()
}

// The Python test goes through Transcriber.transcribe. A Rust Transcriber needs ONNX sessions, so this
// runs the call that transcribe makes first.
#[test]
fn test_a_transcribing_thread_asks_for_full_speed() {
    assert_eq!(power_throttling_after(prefer_performance_cores), (1, 0));
}

#[test]
fn test_a_thread_left_alone_keeps_the_system_default() {
    assert_eq!(power_throttling_after(|| ()), (0, 0));
}

fn seconds(samples: usize) -> f64 {
    Audio { samples: vec![0.0; samples], rate: 16_000 }.seconds()
}

#[test]
fn audio_shorter_than_the_minimum_counts_as_too_short() {
    assert_eq!(MIN_SECONDS, 0.1);
    assert!(seconds(1_599) < MIN_SECONDS);
    assert!(!(seconds(1_600) < MIN_SECONDS));
}

#[test]
fn join_tokens_keeps_a_space_only_before_a_word_character() {
    let cases: [(&[&str], &str); 11] = [
        (&[], ""),
        (&["hel", "lo"], "hello"),
        (&[" hello", " world"], "hello world"),
        (&[" olá", " mundo"], "olá mundo"),
        (&[" hello", ",", " world", "."], "hello, world."),
        (&[" a", " _b"], "a _b"),
        (&[" 2", " 3"], "2 3"),
        (&[" a", " ."], "a."),
        (&[" a", " "], "a"),
        (&[" a", "  b"], "a b"),
        (&["a\tb"], "a b"),
    ];
    for (tokens, want) in cases {
        assert_eq!(join_tokens(tokens), want, "{tokens:?}");
    }
}

#[test]
fn join_tokens_drops_only_the_first_leading_space() {
    assert_eq!(join_tokens(&["\tx"]), "x");
    assert_eq!(join_tokens(&["  a"]), " a");
}

const BLANK: usize = 0;
const TOKEN: usize = 5;

#[test]
fn cursor_stays_on_the_frame_after_a_token_without_a_jump() {
    let mut cursor = Cursor { frame: 3, emitted: 0 };
    cursor.advance(TOKEN, BLANK, 0);
    assert_eq!(cursor, Cursor { frame: 3, emitted: 1 });
}

#[test]
fn cursor_moves_one_frame_on_a_blank() {
    let mut cursor = Cursor { frame: 3, emitted: 2 };
    cursor.advance(BLANK, BLANK, 0);
    assert_eq!(cursor, Cursor { frame: 4, emitted: 0 });
}

#[test]
fn cursor_jumps_by_the_predicted_step() {
    let mut cursor = Cursor { frame: 3, emitted: 2 };
    cursor.advance(TOKEN, BLANK, 4);
    assert_eq!(cursor, Cursor { frame: 7, emitted: 0 });
    cursor.advance(BLANK, BLANK, 2);
    assert_eq!(cursor, Cursor { frame: 9, emitted: 0 });
}

#[test]
fn cursor_moves_on_after_ten_tokens_on_one_frame() {
    let mut cursor = Cursor::default();
    for emitted in 1..10 {
        cursor.advance(TOKEN, BLANK, 0);
        assert_eq!(cursor, Cursor { frame: 0, emitted });
    }
    cursor.advance(TOKEN, BLANK, 0);
    assert_eq!(cursor, Cursor { frame: 1, emitted: 0 });
}

struct Fixed(&'static str);

impl LoadedModel for Fixed {
    fn transcribe(&mut self, audio: &Audio) -> Result<String, String> {
        Ok(format!("{} {}", self.0, audio.rate))
    }
}

fn audio() -> Audio {
    Audio { samples: vec![0.0; 16_000], rate: 16_000 }
}

#[test]
fn shared_model_is_ready_once_loaded_and_runs_what_it_loaded() {
    let shared = SharedModel::default();
    assert!(!shared.ready());
    shared.load(Box::new(Fixed("hello")));
    assert!(shared.ready());
    assert_eq!(shared.transcribe(&audio()).unwrap(), "hello 16000");
}

// Python raises AttributeError on the None here; the Rust model returns the error instead.
#[test]
fn shared_model_refuses_to_transcribe_before_it_loads() {
    assert_eq!(SharedModel::default().transcribe(&audio()).unwrap_err(), "the model is not loaded");
}

#[derive(Default)]
struct Overlap {
    running: AtomicUsize,
    most: AtomicUsize,
}

struct Slow(Arc<Overlap>);

impl LoadedModel for Slow {
    fn transcribe(&mut self, _audio: &Audio) -> Result<String, String> {
        let running = self.0.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.most.fetch_max(running, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(20));
        self.0.running.fetch_sub(1, Ordering::SeqCst);
        Ok(String::new())
    }
}

#[test]
fn shared_model_runs_one_transcription_at_a_time() {
    let overlap = Arc::new(Overlap::default());
    let shared = SharedModel::default();
    shared.load(Box::new(Slow(overlap.clone())));
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| shared.transcribe(&audio()).unwrap());
        }
    });
    assert_eq!(overlap.most.load(Ordering::SeqCst), 1);
}
