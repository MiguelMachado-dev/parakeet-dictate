from parakeet_dictate.reducer import (
    AwaitingRelease,
    Cancel,
    Cue,
    DiscardRecording,
    HotkeyDown,
    HotkeyUp,
    Idle,
    Mode,
    OtherKeyDown,
    Paste,
    PlaySound,
    Recording,
    RecordingFailed,
    StartRecording,
    StopAndTranscribe,
    Transcribing,
    TranscriptReady,
    step,
)

START = [StartRecording(), PlaySound(Cue.START)]
STOP = [StopAndTranscribe(), PlaySound(Cue.STOP)]


def walk(state, script):
    """Feed events one by one, asserting the exact state and effects after each."""
    for i, (event, want_state, want_effects) in enumerate(script):
        state, effects = step(state, event, tap_threshold=0.3)
        assert (state, effects) == (want_state, want_effects), f"step {i}: {event}"
    return state


def test_hold_to_talk_pastes_after_release():
    walk(Idle(), [
        (HotkeyDown(10.0), Recording(Mode.HOLD, 10.0), START),
        (HotkeyUp(11.5), Transcribing(key_down=False), STOP),
        (TranscriptReady("olá mundo"), Idle(), [Paste("olá mundo")]),
    ])


def test_release_exactly_at_threshold_counts_as_hold():
    walk(Idle(), [
        (HotkeyDown(0.0), Recording(Mode.HOLD, 0.0), START),
        (HotkeyUp(0.3), Transcribing(key_down=False), STOP),
    ])


def test_tap_switches_to_toggle_and_next_press_stops():
    walk(Idle(), [
        (HotkeyDown(0.0), Recording(Mode.HOLD, 0.0), START),
        (HotkeyUp(0.12), Recording(Mode.TOGGLE, 0.0), []),
        (HotkeyDown(5.0), Transcribing(key_down=True), STOP),
        (HotkeyUp(5.1), Transcribing(key_down=False), []),
        (TranscriptReady("hands free"), Idle(), [Paste("hands free")]),
    ])


def test_auto_repeat_keydown_is_ignored_while_held():
    walk(Idle(), [
        (HotkeyDown(0.0), Recording(Mode.HOLD, 0.0), START),
        (HotkeyDown(0.5), Recording(Mode.HOLD, 0.0), []),
        (HotkeyDown(0.9), Recording(Mode.HOLD, 0.0), []),
        # Measured from the first press, so this is a hold, not a tap.
        (HotkeyUp(1.0), Transcribing(key_down=False), STOP),
    ])


def test_auto_repeat_after_cancel_does_not_start_a_new_recording():
    walk(Idle(), [
        (HotkeyDown(0.0), Recording(Mode.HOLD, 0.0), START),
        (Cancel(), Idle(key_down=True), [DiscardRecording(), PlaySound(Cue.CANCEL)]),
        (HotkeyDown(0.6), Idle(key_down=True), []),
        (HotkeyUp(0.8), Idle(), []),
        (HotkeyDown(2.0), Recording(Mode.HOLD, 2.0), START),
    ])


def test_esc_cancels_hands_free_recording_without_pasting():
    walk(Idle(), [
        (HotkeyDown(0.0), Recording(Mode.HOLD, 0.0), START),
        (HotkeyUp(0.1), Recording(Mode.TOGGLE, 0.0), []),
        (Cancel(), Idle(), [DiscardRecording(), PlaySound(Cue.CANCEL)]),
        (TranscriptReady("stale"), Idle(), []),
    ])


def test_keys_during_transcribing_are_ignored():
    walk(Transcribing(key_down=False), [
        (HotkeyDown(1.0), Transcribing(key_down=True), []),
        (HotkeyDown(1.4), Transcribing(key_down=True), []),
        (OtherKeyDown(), Transcribing(key_down=True), []),
        (Cancel(), Transcribing(key_down=True), []),
        (HotkeyUp(1.6), Transcribing(key_down=False), []),
        (TranscriptReady("ok"), Idle(), [Paste("ok")]),
    ])


def test_transcript_arriving_while_key_down_pastes_on_key_up():
    walk(Recording(Mode.TOGGLE, 0.0), [
        (HotkeyDown(4.0), Transcribing(key_down=True), STOP),
        (TranscriptReady("later"), AwaitingRelease("later"), []),
        (HotkeyDown(4.3), AwaitingRelease("later"), []),
        (HotkeyUp(4.5), Idle(), [Paste("later")]),
    ])


def test_esc_while_awaiting_release_drops_the_text():
    walk(AwaitingRelease("nope"), [
        (Cancel(), Idle(key_down=True), [PlaySound(Cue.CANCEL)]),
        (HotkeyUp(1.0), Idle(), []),
    ])


def test_empty_transcript_does_not_paste():
    walk(Transcribing(key_down=False), [
        (TranscriptReady(""), Idle(), []),
    ])
    walk(Transcribing(key_down=True), [
        (TranscriptReady(""), Idle(key_down=True), []),
        (HotkeyUp(1.0), Idle(), []),
    ])


def test_recording_failure_returns_to_idle():
    walk(Idle(), [
        (HotkeyDown(0.0), Recording(Mode.HOLD, 0.0), START),
        (RecordingFailed("no mic"), Idle(key_down=True), [DiscardRecording(), PlaySound(Cue.ERROR)]),
        (HotkeyUp(0.5), Idle(), []),
        (HotkeyDown(3.0), Recording(Mode.HOLD, 3.0), START),
    ])


def test_transcription_failure_returns_to_idle():
    walk(Transcribing(key_down=False), [
        (RecordingFailed("cuda error"), Idle(), [PlaySound(Cue.ERROR)]),
    ])


def test_other_key_while_holding_discards_as_a_shortcut():
    walk(Idle(), [
        (HotkeyDown(0.0), Recording(Mode.HOLD, 0.0), START),
        (OtherKeyDown(), Idle(key_down=True), [DiscardRecording()]),
        (HotkeyUp(0.1), Idle(), []),
    ])


def test_typing_during_hands_free_keeps_recording():
    walk(Recording(Mode.TOGGLE, 0.0), [
        (OtherKeyDown(), Recording(Mode.TOGGLE, 0.0), []),
    ])
