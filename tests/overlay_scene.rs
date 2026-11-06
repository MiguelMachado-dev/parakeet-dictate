//! Port of tests/test_overlay_scene.py.

use parakeet_dictate::meter::{LEVEL_HISTORY, SPECTRUM_BANDS};
use parakeet_dictate::overlay::frame::{BEAT_S, FPS, OverlayFrame, Phase};
use parakeet_dictate::overlay::scene::{Command, FADE_IN_S, FADE_OUT_S, Placement, SHOW_DELAY_S, Scene, command_for, place};
use parakeet_dictate::reducer::{Event, IDLE, Transition, step};

const REC: Phase = Phase::Recording;
const TRANSCRIBING: Phase = Phase::Transcribing;
const POLISHING: Phase = Phase::Polishing;
const DONE: Phase = Phase::Done;
const CANCELLED: Phase = Phase::Cancelled;
const FAILED: Phase = Phase::Failed;

static LEVELS: [f32; LEVEL_HISTORY] = [0.0; LEVEL_HISTORY];
static SPECTRUM: [f32; SPECTRUM_BANDS] = [0.0; SPECTRUM_BANDS];

fn here() -> Placement {
    place((0, 0, 2560, 1392), 96)
}

fn show(phase: Phase) -> Option<Command> {
    Some(Command::Show { phase, hands_free: false })
}

fn hands_free(phase: Phase) -> Option<Command> {
    Some(Command::Show { phase, hands_free: true })
}

fn finish(beat: Phase) -> Option<Command> {
    Some(Command::Finish { beat: Some(beat) })
}

fn down(t: f64) -> Event {
    Event::HotkeyDown(t)
}

fn up(t: f64) -> Event {
    Event::HotkeyUp(t)
}

fn transcript(text: &str) -> Event {
    Event::TranscriptReady { text: text.to_owned(), request_id: 1 }
}

/// The overlay command for each state change the reducer makes, as Dictation reports them.
fn commands_with(events: &[Event], polish: bool) -> Vec<Option<Command>> {
    let (mut state, mut out) = (IDLE, Vec::new());
    for event in events {
        let previous = state.clone();
        let (next, effects) = step(&state, event, 0.3, polish);
        state = next;
        if state != previous {
            out.push(command_for(&Transition { previous, state: state.clone(), event: event.clone(), effects }));
        }
    }
    out
}

fn commands(events: &[Event]) -> Vec<Option<Command>> {
    commands_with(events, false)
}

fn assert_approx(actual: f64, expected: f64) {
    let tolerance = (1e-6 * expected.abs()).max(1e-12);
    assert!((actual - expected).abs() <= tolerance, "{actual} is not approximately {expected}");
}

#[test]
fn test_hold_to_talk_records_then_transcribes_then_plays_done() {
    assert_eq!(commands(&[down(0.0), up(2.0), transcript("hi")]), [show(REC), show(TRANSCRIBING), finish(DONE)]);
}

#[test]
fn test_a_tap_turns_the_recording_hands_free() {
    assert_eq!(
        commands(&[down(0.0), up(0.1), down(3.0), up(3.1), transcript("hi")]),
        [show(REC), hands_free(REC), show(TRANSCRIBING), show(TRANSCRIBING), finish(DONE)],
    );
}

#[test]
fn test_polishing_and_awaiting_release() {
    let polished = Event::Polished { request_id: 1, text: "Hi.".to_owned() };
    assert_eq!(
        commands_with(&[down(0.0), up(2.0), transcript("um hi"), polished], true),
        [show(REC), show(TRANSCRIBING), show(POLISHING), finish(DONE)],
    );
    assert_eq!(
        commands(&[down(0.0), up(0.1), down(3.0), transcript("hi"), up(4.0)]),
        [show(REC), hands_free(REC), show(TRANSCRIBING), show(TRANSCRIBING), finish(DONE)],
    );
}

#[test]
fn test_esc_while_polishing_pastes_the_raw_text_so_it_plays_done() {
    let out = commands_with(&[down(0.0), up(2.0), transcript("um hi"), Event::Cancel], true);
    assert_eq!(out.last(), Some(&finish(DONE)));
}

#[test]
fn test_cancel_plays_cancelled() {
    assert_eq!(commands(&[down(0.0), Event::Cancel, up(1.0)]), [show(REC), finish(CANCELLED), None]);
    let awaiting = commands(&[down(0.0), up(0.1), down(3.0), transcript("hi"), Event::Cancel, up(4.0)]);
    assert_eq!(awaiting[awaiting.len() - 2..], [finish(CANCELLED), None]);
}

#[test]
fn test_failures_play_failed() {
    assert_eq!(commands(&[down(0.0), Event::RecordingFailed("no mic".to_owned())]), [show(REC), finish(FAILED)]);
    assert_eq!(
        commands(&[down(0.0), up(2.0), Event::RecordingFailed("cuda".to_owned())]),
        [show(REC), show(TRANSCRIBING), finish(FAILED)],
    );
}

#[test]
fn test_a_shortcut_vanishes_with_no_beat() {
    assert_eq!(commands(&[down(0.0), Event::OtherKeyDown, up(0.2)]), [show(REC), Some(Command::Vanish), None]);
}

#[test]
fn test_an_empty_transcript_fades_with_no_beat() {
    assert_eq!(
        commands(&[down(0.0), up(2.0), transcript("")]),
        [show(REC), show(TRANSCRIBING), Some(Command::Finish { beat: None })],
    );
}

/// `np.arange(start, end, 1 / FPS)`.
fn ticks(start: f64, end: f64) -> Vec<f64> {
    let step = 1.0 / FPS;
    let count = ((end - start) / step).ceil() as usize;
    (0..count).map(|i| start + i as f64 * step).collect()
}

fn frame(scene: &mut Scene, t: f64) -> Option<OverlayFrame<'static>> {
    scene.frame(t, &LEVELS, &SPECTRUM).map(|shown| shown.0)
}

fn shown(scene: &mut Scene, t: f64) -> OverlayFrame<'static> {
    frame(scene, t).expect("the overlay is on screen")
}

fn apply(scene: &mut Scene, command: Option<Command>, t: f64) {
    scene.apply(command.expect("a command"), t, here());
}

#[test]
fn test_a_shortcut_is_never_visible() {
    let mut scene = Scene::default();
    apply(&mut scene, show(REC), 0.0);
    assert!(ticks(0.0, 0.12).into_iter().all(|t| frame(&mut scene, t).is_none()));
    scene.apply(Command::Vanish, 0.12, here());
    assert!(ticks(0.12, 2.0).into_iter().all(|t| frame(&mut scene, t).is_none()));
    assert_eq!(scene.next_show(), None);
}

#[test]
fn test_esc_during_the_show_delay_is_never_visible() {
    let mut scene = Scene::default();
    apply(&mut scene, show(REC), 0.0);
    apply(&mut scene, finish(CANCELLED), 0.1);
    assert!(ticks(0.0, 2.0).into_iter().all(|t| frame(&mut scene, t).is_none()));
}

#[test]
fn test_recording_appears_after_the_delay_and_fades_in() {
    let mut scene = Scene::default();
    apply(&mut scene, show(REC), 10.0);
    assert_approx(scene.next_show().unwrap(), 10.0 + SHOW_DELAY_S);
    assert!(frame(&mut scene, 10.0 + SHOW_DELAY_S - 0.001).is_none());
    // 0.15 s in and 0.2 s out, as in scripts/preview_overlay.py, which the renderers are designed against.
    let half = shown(&mut scene, 10.0 + SHOW_DELAY_S + 0.075);
    assert_eq!(half.phase, REC);
    assert_approx(half.presence, 0.5);
    assert_approx(half.phase_elapsed, 0.075);
    assert_approx(half.recording_elapsed, SHOW_DELAY_S + 0.075);
    assert_eq!(shown(&mut scene, 10.0 + SHOW_DELAY_S + 0.15).presence, 1.0);
    assert_eq!(scene.frame(11.0, &LEVELS, &SPECTRUM).unwrap().1, here());
}

#[test]
fn test_a_beat_plays_then_fades_out() {
    for beat in [DONE, CANCELLED, FAILED] {
        let mut scene = Scene::default();
        apply(&mut scene, show(REC), 0.0);
        frame(&mut scene, 1.0);
        apply(&mut scene, finish(beat), 2.0);
        let playing = shown(&mut scene, 2.0 + BEAT_S - 0.01);
        assert_eq!((playing.phase, playing.presence), (beat, 1.0));
        assert_approx(playing.phase_elapsed, BEAT_S - 0.01);
        let fading = shown(&mut scene, 2.0 + BEAT_S + 0.1);
        assert_eq!(fading.phase, beat);
        assert_approx(fading.presence, 0.5);
        assert!(frame(&mut scene, 2.0 + BEAT_S + 0.2 + 0.001).is_none());
        assert_eq!(scene.next_show(), None);
    }
}

#[test]
fn test_no_beat_fades_out_at_once_in_the_current_phase() {
    let mut scene = Scene::default();
    apply(&mut scene, show(REC), 0.0);
    apply(&mut scene, show(TRANSCRIBING), 1.0);
    scene.apply(Command::Finish { beat: None }, 1.5, here());
    let fading = shown(&mut scene, 1.5 + FADE_OUT_S / 4.0);
    assert_eq!(fading.phase, TRANSCRIBING);
    assert_approx(fading.presence, 0.75);
    assert!(frame(&mut scene, 1.5 + FADE_OUT_S + 0.001).is_none());
}

#[test]
fn test_a_shortcut_after_the_window_appeared_hides_it_at_once() {
    let mut scene = Scene::default();
    apply(&mut scene, show(REC), 0.0);
    assert_eq!(shown(&mut scene, 0.6).presence, 1.0);
    scene.apply(Command::Vanish, 0.7, here());
    assert!(frame(&mut scene, 0.7).is_none());
}

#[test]
fn test_hands_free_sticks_and_recording_time_freezes_when_recording_stops() {
    let mut scene = Scene::default();
    apply(&mut scene, show(REC), 0.0);
    frame(&mut scene, SHOW_DELAY_S + 0.01);
    apply(&mut scene, hands_free(REC), SHOW_DELAY_S + 0.02);
    let recording = shown(&mut scene, 1.0);
    assert!(recording.hands_free);
    assert_approx(recording.phase_elapsed, 1.0 - SHOW_DELAY_S);
    apply(&mut scene, show(TRANSCRIBING), 2.0);
    let transcribing = shown(&mut scene, 2.5);
    assert_eq!((transcribing.phase, transcribing.hands_free), (TRANSCRIBING, true));
    assert_approx(transcribing.phase_elapsed, 0.5);
    assert_approx(transcribing.recording_elapsed, 2.0);
}

#[test]
fn test_a_new_recording_during_a_fade_out_rises_from_where_the_fade_was() {
    let mut scene = Scene::default();
    let elsewhere = place((-2560, 0, 0, 1392), 96);
    apply(&mut scene, show(REC), 0.0);
    apply(&mut scene, finish(DONE), 1.0);
    scene.apply(Command::Show { phase: REC, hands_free: false }, 1.3, elsewhere);
    let old = shown(&mut scene, 1.3 + SHOW_DELAY_S - 0.01);
    assert_eq!(old.phase, DONE);
    let fade_progress = (1.3 + SHOW_DELAY_S - (1.0 + BEAT_S)) / FADE_OUT_S;
    let (new, place) = scene.frame(1.3 + SHOW_DELAY_S, &LEVELS, &SPECTRUM).unwrap();
    assert_eq!((new.phase, place), (REC, elsewhere));
    assert_approx(new.presence, 1.0 - fade_progress);
    assert_approx(shown(&mut scene, 1.3 + SHOW_DELAY_S + FADE_IN_S * fade_progress).presence, 1.0);
}

#[test]
fn test_a_shortcut_during_the_previous_beat_leaves_the_beat_alone() {
    let mut scene = Scene::default();
    apply(&mut scene, show(REC), 0.0);
    apply(&mut scene, finish(DONE), 1.0);
    apply(&mut scene, show(REC), 1.1);
    scene.apply(Command::Vanish, 1.2, here());
    let playing = shown(&mut scene, 1.3);
    assert_eq!((playing.phase, playing.presence), (DONE, 1.0));
    assert!(frame(&mut scene, 1.0 + BEAT_S + FADE_OUT_S + 0.001).is_none());
    assert_eq!(scene.next_show(), None);
}

#[test]
fn test_placement_is_bottom_center_of_the_work_area_on_a_monitor_left_of_the_primary() {
    assert_eq!(place((-2560, 0, 0, 1392), 96), Placement { x: -1480, y: 1392 - 48 - 120, width: 400, height: 120, scale: 1.0 });
}

#[test]
fn test_placement_scales_with_the_monitor_dpi() {
    let expected = Placement { x: (3840 - 600) / 2, y: 2112 - 72 - 180, width: 600, height: 180, scale: 1.5 };
    assert_eq!(place((0, 0, 3840, 2112), 144), expected);
}
