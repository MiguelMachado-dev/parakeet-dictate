//! Port of tests/test_reducer.py.

use parakeet_dictate::reducer::{Cue, Effect, Event, IDLE, Mode, State, step};

const KEY_HELD_IDLE: State = State::Idle { key_down: true };
const RAW: &str = "um so the meeting is friday";

type Script = Vec<(Event, State, Vec<Effect>)>;

fn start() -> Vec<Effect> {
    vec![Effect::StartRecording, Effect::PlaySound(Cue::Start)]
}

fn stop() -> Vec<Effect> {
    vec![Effect::StopAndTranscribe, Effect::PlaySound(Cue::Stop)]
}

fn cancelled() -> Vec<Effect> {
    vec![Effect::DiscardRecording, Effect::PlaySound(Cue::Cancel)]
}

fn hold(pressed_at: f64) -> State {
    State::Recording { mode: Mode::Hold, pressed_at }
}

fn toggle(pressed_at: f64) -> State {
    State::Recording { mode: Mode::Toggle, pressed_at }
}

fn transcribing(key_down: bool) -> State {
    State::Transcribing { key_down }
}

fn polishing(raw: &str, request_id: u64, key_down: bool) -> State {
    State::Polishing { raw: raw.to_owned(), request_id, key_down }
}

fn awaiting(text: &str) -> State {
    State::AwaitingRelease { text: text.to_owned() }
}

fn down(t: f64) -> Event {
    Event::HotkeyDown(t)
}

fn up(t: f64) -> Event {
    Event::HotkeyUp(t)
}

fn ready(text: &str, request_id: u64) -> Event {
    Event::TranscriptReady { text: text.to_owned(), request_id }
}

fn failed(error: &str) -> Event {
    Event::RecordingFailed(error.to_owned())
}

fn polished(request_id: u64, text: &str) -> Event {
    Event::Polished { request_id, text: text.to_owned() }
}

fn polish_failed(request_id: u64, reason: &str) -> Event {
    Event::PolishFailed { request_id, reason: reason.to_owned() }
}

fn paste(text: &str) -> Vec<Effect> {
    vec![Effect::Paste(text.to_owned())]
}

fn polish(text: &str, request_id: u64) -> Vec<Effect> {
    vec![Effect::Polish { text: text.to_owned(), request_id }]
}

fn run(mut state: State, script: Script, polish: bool) -> State {
    for (i, (event, want_state, want_effects)) in script.into_iter().enumerate() {
        let (next, effects) = step(&state, &event, 0.3, polish);
        assert_eq!((&next, &effects), (&want_state, &want_effects), "step {i}: {event:?}");
        state = next;
    }
    state
}

fn walk(state: State, script: Script) -> State {
    run(state, script, false)
}

fn walk_polishing(state: State, script: Script) -> State {
    run(state, script, true)
}

#[test]
fn test_hold_to_talk_pastes_after_release() {
    walk(
        IDLE,
        vec![
            (down(10.0), hold(10.0), start()),
            (up(11.5), transcribing(false), stop()),
            (ready("olá mundo", 1), IDLE, paste("olá mundo")),
        ],
    );
}

#[test]
fn test_release_exactly_at_threshold_counts_as_hold() {
    walk(IDLE, vec![(down(0.0), hold(0.0), start()), (up(0.3), transcribing(false), stop())]);
}

#[test]
fn test_tap_switches_to_toggle_and_next_press_stops() {
    walk(
        IDLE,
        vec![
            (down(0.0), hold(0.0), start()),
            (up(0.12), toggle(0.0), vec![]),
            (down(5.0), transcribing(true), stop()),
            (up(5.1), transcribing(false), vec![]),
            (ready("hands free", 1), IDLE, paste("hands free")),
        ],
    );
}

#[test]
fn test_auto_repeat_keydown_is_ignored_while_held() {
    walk(
        IDLE,
        vec![
            (down(0.0), hold(0.0), start()),
            (down(0.5), hold(0.0), vec![]),
            (down(0.9), hold(0.0), vec![]),
            (up(1.0), transcribing(false), stop()),
        ],
    );
}

#[test]
fn test_auto_repeat_after_cancel_does_not_start_a_new_recording() {
    walk(
        IDLE,
        vec![
            (down(0.0), hold(0.0), start()),
            (Event::Cancel, KEY_HELD_IDLE, cancelled()),
            (down(0.6), KEY_HELD_IDLE, vec![]),
            (up(0.8), IDLE, vec![]),
            (down(2.0), hold(2.0), start()),
        ],
    );
}

#[test]
fn test_esc_cancels_hands_free_recording_without_pasting() {
    walk(
        IDLE,
        vec![
            (down(0.0), hold(0.0), start()),
            (up(0.1), toggle(0.0), vec![]),
            (Event::Cancel, IDLE, cancelled()),
            (ready("stale", 1), IDLE, vec![]),
        ],
    );
}

#[test]
fn test_keys_during_transcribing_are_ignored() {
    walk(
        transcribing(false),
        vec![
            (down(1.0), transcribing(true), vec![]),
            (down(1.4), transcribing(true), vec![]),
            (Event::OtherKeyDown, transcribing(true), vec![]),
            (Event::Cancel, transcribing(true), vec![]),
            (up(1.6), transcribing(false), vec![]),
            (ready("ok", 1), IDLE, paste("ok")),
        ],
    );
}

#[test]
fn test_transcript_arriving_while_key_down_pastes_on_key_up() {
    walk(
        toggle(0.0),
        vec![
            (down(4.0), transcribing(true), stop()),
            (ready("later", 1), awaiting("later"), vec![]),
            (down(4.3), awaiting("later"), vec![]),
            (up(4.5), IDLE, paste("later")),
        ],
    );
}

#[test]
fn test_esc_while_awaiting_release_drops_the_text() {
    walk(
        awaiting("nope"),
        vec![(Event::Cancel, KEY_HELD_IDLE, vec![Effect::PlaySound(Cue::Cancel)]), (up(1.0), IDLE, vec![])],
    );
}

#[test]
fn test_empty_transcript_does_not_paste() {
    walk(transcribing(false), vec![(ready("", 1), IDLE, vec![])]);
    walk(transcribing(true), vec![(ready("", 1), KEY_HELD_IDLE, vec![]), (up(1.0), IDLE, vec![])]);
}

#[test]
fn test_recording_failure_returns_to_idle() {
    walk(
        IDLE,
        vec![
            (down(0.0), hold(0.0), start()),
            (failed("no mic"), KEY_HELD_IDLE, vec![Effect::DiscardRecording, Effect::PlaySound(Cue::Error)]),
            (up(0.5), IDLE, vec![]),
            (down(3.0), hold(3.0), start()),
        ],
    );
}

#[test]
fn test_transcription_failure_returns_to_idle() {
    walk(transcribing(false), vec![(failed("cuda error"), IDLE, vec![Effect::PlaySound(Cue::Error)])]);
}

#[test]
fn test_other_key_while_holding_discards_as_a_shortcut() {
    walk(
        IDLE,
        vec![
            (down(0.0), hold(0.0), start()),
            (Event::OtherKeyDown, KEY_HELD_IDLE, vec![Effect::DiscardRecording]),
            (up(0.1), IDLE, vec![]),
        ],
    );
}

#[test]
fn test_typing_during_hands_free_keeps_recording() {
    walk(toggle(0.0), vec![(Event::OtherKeyDown, toggle(0.0), vec![])]);
}

#[test]
fn test_post_processing_pastes_the_polished_text() {
    walk_polishing(
        IDLE,
        vec![
            (down(0.0), hold(0.0), start()),
            (up(2.0), transcribing(false), stop()),
            (ready(RAW, 7), polishing(RAW, 7, false), polish(RAW, 7)),
            (polished(7, "The meeting is Friday."), IDLE, paste("The meeting is Friday.")),
        ],
    );
}

#[test]
fn test_post_processing_off_pastes_the_transcript_directly() {
    walk(transcribing(false), vec![(ready(RAW, 7), IDLE, paste(RAW))]);
}

#[test]
fn test_empty_transcript_skips_post_processing() {
    walk_polishing(transcribing(false), vec![(ready("", 7), IDLE, vec![])]);
    walk_polishing(transcribing(true), vec![(ready("", 7), KEY_HELD_IDLE, vec![])]);
}

#[test]
fn test_polished_text_arriving_while_key_down_pastes_on_key_up() {
    walk_polishing(
        transcribing(true),
        vec![
            (ready(RAW, 7), polishing(RAW, 7, true), polish(RAW, 7)),
            (polished(7, "Friday."), awaiting("Friday."), vec![]),
            (up(9.0), IDLE, paste("Friday.")),
        ],
    );
}

#[test]
fn test_hotkey_during_polishing_only_tracks_the_key() {
    walk_polishing(
        polishing(RAW, 7, false),
        vec![
            (down(1.0), polishing(RAW, 7, true), vec![]),
            (down(1.4), polishing(RAW, 7, true), vec![]),
            (Event::OtherKeyDown, polishing(RAW, 7, true), vec![]),
            (up(1.6), polishing(RAW, 7, false), vec![]),
            (ready("ignored", 8), polishing(RAW, 7, false), vec![]),
            (polished(7, "Friday."), IDLE, paste("Friday.")),
        ],
    );
}

#[test]
fn test_failure_pastes_the_raw_transcript() {
    walk(polishing(RAW, 7, false), vec![(polish_failed(7, "timeout"), IDLE, paste(RAW))]);
    walk(
        polishing(RAW, 7, true),
        vec![(polish_failed(7, "HTTP 500"), awaiting(RAW), vec![]), (up(3.0), IDLE, paste(RAW))],
    );
}

#[test]
fn test_esc_while_polishing_pastes_the_raw_transcript_and_ignores_the_late_reply() {
    walk(
        polishing(RAW, 7, false),
        vec![
            (Event::Cancel, IDLE, paste(RAW)),
            (polished(7, "Friday."), IDLE, vec![]),
            (polish_failed(7, "timeout"), IDLE, vec![]),
        ],
    );
    walk(polishing(RAW, 7, true), vec![(Event::Cancel, awaiting(RAW), vec![]), (up(3.0), IDLE, paste(RAW))]);
}

#[test]
fn test_result_for_another_request_is_ignored_while_polishing() {
    let second = polishing("second dictation", 8, false);
    walk(
        second.clone(),
        vec![
            (polished(7, "First."), second.clone(), vec![]),
            (polish_failed(7, "timeout"), second, vec![]),
            (polished(8, "Second dictation."), IDLE, paste("Second dictation.")),
        ],
    );
}

#[test]
fn test_late_result_is_ignored_in_every_other_state() {
    for state in [IDLE, hold(0.0), transcribing(false), awaiting("text")] {
        walk_polishing(
            state.clone(),
            vec![(polished(7, "late"), state.clone(), vec![]), (polish_failed(7, "late"), state, vec![])],
        );
    }
}
