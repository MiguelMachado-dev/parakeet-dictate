import json
import threading
from queue import Queue
from types import SimpleNamespace

import httpx2
import numpy as np
import pytest

from parakeet_dictate.app import Cues, Dictation
from parakeet_dictate.app_rules import AppRule
from parakeet_dictate.config import Config, PostprocessConfig
from parakeet_dictate.polish import create_polisher
from parakeet_dictate.recorder import Audio
from parakeet_dictate.reducer import (
    AwaitingRelease,
    Cancel,
    Cue,
    HotkeyDown,
    HotkeyUp,
    Idle,
    Paste,
    PlaySound,
    Polishing,
    Recording,
    StartRecording,
    StopAndTranscribe,
    Transcribing,
)


class FakeRecorder:
    device_name = "fixture"

    def __init__(self) -> None:
        self.started = threading.Event()

    def start(self) -> None:
        self.started.set()

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


class GatedTranscriber:
    def __init__(self) -> None:
        self.entered = threading.Event()
        self.release = threading.Event()
        self.release.set()

    def transcribe(self, audio: Audio) -> str:
        self.entered.set()
        assert self.release.wait(5), "transcription was not released"
        return "hi"


class FakePolisher:
    def __init__(self) -> None:
        self.requests = Queue()
        self.preconnected = threading.Event()

    def preconnect(self) -> None:
        self.preconnected.set()

    def polish(self, raw: str, instructions: str = "") -> str:
        self.requests.put((raw, instructions))
        return "polished"

    def close(self) -> None:
        pass


RULES = (
    AppRule("English", "Translate to English.", ("Slack.exe",)),
    AppRule("Formal", "Be formal.", ("notepad.exe",)),
)


@pytest.fixture
def running():
    instances = []

    def start(*, rules=RULES, enabled=False, with_key=True, polisher=None, postprocess=PostprocessConfig()):
        client = polisher if polisher is not None else FakePolisher() if with_key else None
        h = SimpleNamespace(
            front=["Slack.exe"], recorder=FakeRecorder(), transcriber=GatedTranscriber(),
            polisher=client, pasted=Queue(), transitions=Queue(), enabled=enabled,
        )
        h.dictation = Dictation(
            Config(app_rules=rules, postprocess=postprocess, append_space=False),
            recorder=h.recorder, transcriber=h.transcriber, polisher=client, cues=Cues(enabled=False),
            paste=h.pasted.put, on_transition=h.transitions.put, polish_enabled=lambda: h.enabled,
            foreground_app=lambda: h.front[0],
        )
        h.dictation._controller.start()
        instances.append(h)
        return h

    yield start
    for h in instances:
        h.transcriber.release.set()
        h.dictation.stop()
        if h.polisher is not None:
            h.polisher.close()


def begin(h):
    h.recorder.started.clear()
    h.dictation.post(HotkeyDown(0))
    assert h.recorder.started.wait(5)


def wait_state(h, state_type):
    while True:
        state = h.transitions.get(timeout=5).state
        if isinstance(state, state_type):
            return state


def test_rules_use_the_app_at_recording_end_and_do_not_follow_focus_during_transcription(running):
    h = running()
    h.front[0] = "notepad.exe"
    h.transcriber.release.clear()
    begin(h)
    h.front[0] = "SLACK.EXE"
    h.dictation.post(HotkeyUp(2))
    assert h.transcriber.entered.wait(5)
    h.front[0] = "notepad.exe"
    h.transcriber.release.set()
    assert h.pasted.get(timeout=5) == "polished"
    assert h.polisher.requests.get(timeout=5) == ("hi", "Translate to English.")
    assert h.enabled is False


def test_a_rule_does_not_leak_into_the_next_dictation_without_a_match(running):
    h = running()
    begin(h)
    h.dictation.post(HotkeyUp(2))
    assert h.pasted.get(timeout=5) == "polished"
    assert h.polisher.requests.get(timeout=5) == ("hi", "Translate to English.")
    h.front[0] = "unknown.exe"
    begin(h)
    h.dictation.post(HotkeyUp(2))
    assert h.pasted.get(timeout=5) == "hi"
    assert h.polisher.requests.empty()


@pytest.mark.parametrize("front", [None, "unmatched.exe", "slack.exe"])
def test_unmatched_disabled_and_empty_rules_leave_raw_dictation_local_when_toggle_is_off(running, front):
    rules = (AppRule("Off", "Translate.", ("slack.exe",), enabled=False),
             AppRule("Empty", " \n", ("slack.exe",)))
    h = running(rules=rules)
    h.front[0] = front
    begin(h)
    h.dictation.post(HotkeyUp(2))
    assert h.pasted.get(timeout=5) == "hi"
    assert h.polisher.requests.empty() and not h.polisher.preconnected.is_set()


def test_a_matching_rule_without_a_provider_key_pastes_raw_and_explains_why(running, caplog):
    h = running(with_key=False, postprocess=PostprocessConfig(provider="anthropic"))
    begin(h)
    h.dictation.post(HotkeyUp(2))
    assert h.pasted.get(timeout=5) == "hi"
    assert "app rules need ANTHROPIC_API_KEY" in caplog.text
    assert "Translate to English" not in caplog.text


@pytest.mark.parametrize("provider", ["zai", "anthropic"])
def test_local_rules_reach_the_selected_client_even_with_postprocessing_off(running, provider):
    requests = []

    def handler(request):
        if request.method == "OPTIONS":
            return httpx2.Response(200)
        body = json.loads(request.content)
        requests.append(body)
        if provider == "anthropic":
            return httpx2.Response(200, json={"content": [{"type": "text", "text": "Hello."}], "stop_reason": "end_turn"})
        return httpx2.Response(200, json={"choices": [{"message": {"content": "Hello."}, "finish_reason": "stop"}]})

    config = PostprocessConfig(provider=provider)
    client = create_polisher(config, "test-key", transport=httpx2.MockTransport(handler))
    h = running(polisher=client, postprocess=config)
    begin(h)
    h.dictation.post(HotkeyUp(2))
    assert h.pasted.get(timeout=5) == "Hello."
    body, = requests
    system = body["system"] if provider == "anthropic" else body["messages"][0]["content"]
    assert system.endswith("\n\nTranslate to English.")


def test_a_rule_in_hands_free_mode_still_waits_for_hotkey_release(running):
    h = running()
    begin(h)
    wait_state(h, Recording)
    h.dictation.post(HotkeyUp(0.1))
    wait_state(h, Recording)
    h.dictation.post(HotkeyDown(2))
    wait_state(h, AwaitingRelease)
    assert h.pasted.empty()
    h.dictation.post(HotkeyUp(3))
    assert h.pasted.get(timeout=5) == "polished"


@pytest.mark.parametrize("skip", ["escape", "timeout"])
def test_queued_requests_keep_their_rules_after_raw_fallback_and_a_new_dictation(running, monkeypatch, skip):
    h = running(postprocess=PostprocessConfig(timeout_s=0.5 if skip == "timeout" else 8))
    barrier, release = threading.Barrier(3), threading.Event()

    def block():
        barrier.wait(timeout=5)
        release.wait(5)

    for _ in range(2):
        h.dictation._polisher.submit(block)
    barrier.wait(timeout=5)
    submit, futures = h.dictation._polisher.submit, []

    def tracked_submit(*args, **kwargs):
        future = submit(*args, **kwargs)
        futures.append(future)
        return future

    monkeypatch.setattr(h.dictation._polisher, "submit", tracked_submit)
    try:
        begin(h)
        h.dictation.post(HotkeyUp(2))
        wait_state(h, Polishing)
        if skip == "escape":
            h.dictation.post(Cancel())
        assert h.pasted.get(timeout=5) == "hi"

        h.front[0] = "notepad.exe"
        begin(h)
        h.dictation.post(HotkeyUp(2))
        wait_state(h, Polishing)
        release.set()
        assert h.pasted.get(timeout=5) == "polished"
        for future in list(futures):
            future.result(timeout=5)
        assert sorted([h.polisher.requests.get(timeout=5), h.polisher.requests.get(timeout=5)]) == [
            ("hi", "Be formal."), ("hi", "Translate to English."),
        ]
        # A new recording starts only after the controller consumes every late reply above.
        begin(h)
        assert h.pasted.empty()
    finally:
        release.set()
