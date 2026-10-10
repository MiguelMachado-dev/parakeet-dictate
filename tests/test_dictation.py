import threading

import numpy as np

from parakeet_dictate.app import Cues, Dictation
from parakeet_dictate.config import Config
from parakeet_dictate.recorder import Audio
from parakeet_dictate.reducer import (
    Cue,
    HotkeyDown,
    HotkeyUp,
    Idle,
    Paste,
    PlaySound,
    Recording,
    StartRecording,
    StopAndTranscribe,
    Transcribing,
)


class FakeRecorder:
    def start(self) -> None:
        pass

    def stop(self) -> Audio:
        return Audio(np.zeros(16_000, np.float32), 16_000)

    def discard(self) -> None:
        pass


class FakeTranscriber:
    def transcribe(self, audio: Audio) -> str:
        return "hi"


def test_the_observer_sees_each_transition_with_its_effects_before_they_run():
    seen, seen_at_paste, pasted = [], [], threading.Event()

    def paste(text: str) -> None:
        seen_at_paste.extend(seen)
        pasted.set()

    dictation = Dictation(
        Config(), recorder=FakeRecorder(), transcriber=FakeTranscriber(), paste=paste, cues=Cues(enabled=False),
        on_transition=seen.append,
    )
    dictation._controller.start()
    try:
        dictation.post(HotkeyDown(0.0))
        dictation.post(HotkeyUp(2.0))
        assert pasted.wait(5)
    finally:
        dictation.stop()

    assert [(type(t.previous), type(t.state), t.effects) for t in seen] == [
        (Idle, Recording, (StartRecording(), PlaySound(Cue.START))),
        (Recording, Transcribing, (StopAndTranscribe(), PlaySound(Cue.STOP))),
        (Transcribing, Idle, (Paste("hi"),)),
    ]
    assert seen_at_paste == seen
