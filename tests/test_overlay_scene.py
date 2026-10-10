import numpy as np
import pytest

from parakeet_dictate.meter import LEVEL_HISTORY, SPECTRUM_BANDS
from parakeet_dictate.overlay_frame import BEAT_S, FPS, Phase
from parakeet_dictate.overlay_scene import (
    FADE_IN_S,
    FADE_OUT_S,
    SHOW_DELAY_S,
    Finish,
    Placement,
    Scene,
    Show,
    Vanish,
    command_for,
    place,
)
from parakeet_dictate.reducer import (
    Cancel,
    HotkeyDown,
    HotkeyUp,
    Idle,
    OtherKeyDown,
    Polished,
    RecordingFailed,
    TranscriptReady,
    Transition,
    step,
)

REC, TRANSCRIBING, POLISHING = Phase.RECORDING, Phase.TRANSCRIBING, Phase.POLISHING
DONE, CANCELLED, FAILED = Phase.DONE, Phase.CANCELLED, Phase.FAILED
HERE = place((0, 0, 2560, 1392), 96)
LEVELS = np.zeros(LEVEL_HISTORY, np.float32), np.zeros(SPECTRUM_BANDS, np.float32)


def commands(events, polish=False):
    """The overlay command for each state change the reducer makes, as Dictation reports them."""
    state, out = Idle(), []
    for event in events:
        previous = state
        state, effects = step(state, event, tap_threshold=0.3, polish=polish)
        if state != previous:
            out.append(command_for(Transition(previous, state, event, tuple(effects))))
    return out


def test_hold_to_talk_records_then_transcribes_then_plays_done():
    assert commands([HotkeyDown(0.0), HotkeyUp(2.0), TranscriptReady("hi", 1)]) == [
        Show(REC), Show(TRANSCRIBING), Finish(DONE),
    ]


def test_a_tap_turns_the_recording_hands_free():
    assert commands([HotkeyDown(0.0), HotkeyUp(0.1), HotkeyDown(3.0), HotkeyUp(3.1), TranscriptReady("hi", 1)]) == [
        Show(REC), Show(REC, hands_free=True), Show(TRANSCRIBING), Show(TRANSCRIBING), Finish(DONE),
    ]


def test_polishing_and_awaiting_release():
    assert commands([HotkeyDown(0.0), HotkeyUp(2.0), TranscriptReady("um hi", 1), Polished(1, "Hi.")], polish=True) == [
        Show(REC), Show(TRANSCRIBING), Show(POLISHING), Finish(DONE),
    ]
    assert commands([HotkeyDown(0.0), HotkeyUp(0.1), HotkeyDown(3.0), TranscriptReady("hi", 1), HotkeyUp(4.0)]) == [
        Show(REC), Show(REC, hands_free=True), Show(TRANSCRIBING), Show(TRANSCRIBING), Finish(DONE),
    ]


def test_esc_while_polishing_pastes_the_raw_text_so_it_plays_done():
    assert commands([HotkeyDown(0.0), HotkeyUp(2.0), TranscriptReady("um hi", 1), Cancel()], polish=True)[-1] == Finish(DONE)


def test_cancel_plays_cancelled():
    assert commands([HotkeyDown(0.0), Cancel(), HotkeyUp(1.0)]) == [Show(REC), Finish(CANCELLED), None]
    awaiting = [HotkeyDown(0.0), HotkeyUp(0.1), HotkeyDown(3.0), TranscriptReady("hi", 1), Cancel(), HotkeyUp(4.0)]
    assert commands(awaiting)[-2:] == [Finish(CANCELLED), None]


def test_failures_play_failed():
    assert commands([HotkeyDown(0.0), RecordingFailed("no mic")]) == [Show(REC), Finish(FAILED)]
    assert commands([HotkeyDown(0.0), HotkeyUp(2.0), RecordingFailed("cuda")]) == [Show(REC), Show(TRANSCRIBING), Finish(FAILED)]


def test_a_shortcut_vanishes_with_no_beat():
    assert commands([HotkeyDown(0.0), OtherKeyDown(), HotkeyUp(0.2)]) == [Show(REC), Vanish(), None]


def test_an_empty_transcript_fades_with_no_beat():
    assert commands([HotkeyDown(0.0), HotkeyUp(2.0), TranscriptReady("", 1)]) == [Show(REC), Show(TRANSCRIBING), Finish(None)]


def ticks(start, end):
    return np.arange(start, end, 1 / FPS)


def frame(scene, t):
    shown = scene.frame(t, *LEVELS)
    return None if shown is None else shown[0]


def test_a_shortcut_is_never_visible():
    scene = Scene()
    scene.apply(Show(REC), 0.0, HERE)
    assert [frame(scene, t) for t in ticks(0.0, 0.12)] == [None] * len(ticks(0.0, 0.12))
    scene.apply(Vanish(), 0.12, HERE)
    assert all(frame(scene, t) is None for t in ticks(0.12, 2.0))
    assert scene.next_show() is None


def test_esc_during_the_show_delay_is_never_visible():
    scene = Scene()
    scene.apply(Show(REC), 0.0, HERE)
    scene.apply(Finish(CANCELLED), 0.1, HERE)
    assert all(frame(scene, t) is None for t in ticks(0.0, 2.0))


def test_recording_appears_after_the_delay_and_fades_in():
    scene = Scene()
    scene.apply(Show(REC), 10.0, HERE)
    assert scene.next_show() == pytest.approx(10.0 + SHOW_DELAY_S)
    assert frame(scene, 10.0 + SHOW_DELAY_S - 0.001) is None
    # 0.15 s in and 0.2 s out, as in scripts/preview_overlay.py, which the renderers are designed against.
    half = frame(scene, 10.0 + SHOW_DELAY_S + 0.075)
    assert (half.phase, half.presence) == (REC, pytest.approx(0.5))
    assert half.phase_elapsed == pytest.approx(0.075)
    assert half.recording_elapsed == pytest.approx(SHOW_DELAY_S + 0.075)
    assert frame(scene, 10.0 + SHOW_DELAY_S + 0.15).presence == 1.0
    assert scene.frame(11.0, *LEVELS)[1] == HERE


@pytest.mark.parametrize("beat", [DONE, CANCELLED, FAILED])
def test_a_beat_plays_then_fades_out(beat):
    scene = Scene()
    scene.apply(Show(REC), 0.0, HERE)
    frame(scene, 1.0)
    scene.apply(Finish(beat), 2.0, HERE)
    playing = frame(scene, 2.0 + BEAT_S - 0.01)
    assert (playing.phase, playing.presence, playing.phase_elapsed) == (beat, 1.0, pytest.approx(BEAT_S - 0.01))
    fading = frame(scene, 2.0 + BEAT_S + 0.1)
    assert (fading.phase, fading.presence) == (beat, pytest.approx(0.5))
    assert frame(scene, 2.0 + BEAT_S + 0.2 + 0.001) is None
    assert scene.next_show() is None


def test_no_beat_fades_out_at_once_in_the_current_phase():
    scene = Scene()
    scene.apply(Show(REC), 0.0, HERE)
    scene.apply(Show(TRANSCRIBING), 1.0, HERE)
    scene.apply(Finish(None), 1.5, HERE)
    fading = frame(scene, 1.5 + FADE_OUT_S / 4)
    assert (fading.phase, fading.presence) == (TRANSCRIBING, pytest.approx(0.75))
    assert frame(scene, 1.5 + FADE_OUT_S + 0.001) is None


def test_a_shortcut_after_the_window_appeared_hides_it_at_once():
    scene = Scene()
    scene.apply(Show(REC), 0.0, HERE)
    assert frame(scene, 0.6).presence == 1.0
    scene.apply(Vanish(), 0.7, HERE)
    assert frame(scene, 0.7) is None


def test_hands_free_sticks_and_recording_time_freezes_when_recording_stops():
    scene = Scene()
    scene.apply(Show(REC), 0.0, HERE)
    frame(scene, SHOW_DELAY_S + 0.01)
    scene.apply(Show(REC, hands_free=True), SHOW_DELAY_S + 0.02, HERE)
    recording = frame(scene, 1.0)
    assert (recording.hands_free, recording.phase_elapsed) == (True, pytest.approx(1.0 - SHOW_DELAY_S))
    scene.apply(Show(TRANSCRIBING), 2.0, HERE)
    transcribing = frame(scene, 2.5)
    assert (transcribing.phase, transcribing.hands_free) == (TRANSCRIBING, True)
    assert transcribing.phase_elapsed == pytest.approx(0.5)
    assert transcribing.recording_elapsed == pytest.approx(2.0)


def test_a_new_recording_during_a_fade_out_rises_from_where_the_fade_was():
    scene = Scene()
    elsewhere = place((-2560, 0, 0, 1392), 96)
    scene.apply(Show(REC), 0.0, HERE)
    scene.apply(Finish(DONE), 1.0, HERE)
    scene.apply(Show(REC), 1.3, elsewhere)
    old = frame(scene, 1.3 + SHOW_DELAY_S - 0.01)
    assert old.phase is DONE
    fade_progress = (1.3 + SHOW_DELAY_S - (1.0 + BEAT_S)) / FADE_OUT_S
    new, where = scene.frame(1.3 + SHOW_DELAY_S, *LEVELS)
    assert (new.phase, new.presence, where) == (REC, pytest.approx(1 - fade_progress), elsewhere)
    assert frame(scene, 1.3 + SHOW_DELAY_S + FADE_IN_S * fade_progress).presence == pytest.approx(1.0)


def test_a_shortcut_during_the_previous_beat_leaves_the_beat_alone():
    scene = Scene()
    scene.apply(Show(REC), 0.0, HERE)
    scene.apply(Finish(DONE), 1.0, HERE)
    scene.apply(Show(REC), 1.1, HERE)
    scene.apply(Vanish(), 1.2, HERE)
    playing = frame(scene, 1.3)
    assert (playing.phase, playing.presence) == (DONE, 1.0)
    assert frame(scene, 1.0 + BEAT_S + FADE_OUT_S + 0.001) is None
    assert scene.next_show() is None


def test_placement_is_bottom_center_of_the_work_area_on_a_monitor_left_of_the_primary():
    assert place((-2560, 0, 0, 1392), 96) == Placement(-1480, 1392 - 48 - 120, 400, 120, 1.0)


def test_placement_scales_with_the_monitor_dpi():
    assert place((0, 0, 3840, 2112), 144) == Placement((3840 - 600) // 2, 2112 - 72 - 180, 600, 180, 1.5)
