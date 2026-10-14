import http.client
import io
import json
import struct
import threading
import time
import wave

import httpx2
import numpy as np
import pytest

from parakeet_dictate.app import Cues, Dictation
from parakeet_dictate.config import Config, PostprocessConfig, ServerConfig
from parakeet_dictate.polish import PolishError, create_polisher
from parakeet_dictate.recorder import Audio
from parakeet_dictate.reducer import HotkeyDown, HotkeyUp
from parakeet_dictate.server import MAX_BODY_BYTES, DictationServer
from parakeet_dictate.transcriber import SharedModel

RAW = "um so the meeting is at three"
POLISHED = "So the meeting is at three."


class FakeTranscriber:
    """Records when each transcription entered and left, to prove two never overlap."""

    def __init__(self, text: str = RAW, seconds: float = 0.0) -> None:
        self.text = text
        self.seconds = seconds
        self.audio: list[Audio] = []
        self.spans: list[tuple[float, float]] = []
        self.entered = threading.Event()

    def transcribe(self, audio: Audio) -> str:
        enter = time.perf_counter()
        self.entered.set()
        time.sleep(self.seconds)
        self.audio.append(audio)
        self.spans.append((enter, time.perf_counter()))
        return self.text


class FakeGlm:
    def __init__(self, reply: str = POLISHED, error: Exception | None = None, seconds: float = 0.0) -> None:
        self.reply, self.error, self.seconds = reply, error, seconds
        self.polished: list[str] = []
        self.instructions: list[str] = []
        self.preconnected = threading.Event()

    def preconnect(self) -> None:
        self.preconnected.set()

    def polish(self, raw: str, instructions: str = "") -> str:
        self.polished.append(raw)
        self.instructions.append(instructions)
        time.sleep(self.seconds)
        if self.error:
            raise self.error
        return self.reply


def wav(samples: np.ndarray, rate: int = 16_000, channels: int = 1) -> bytes:
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as f:
        f.setnchannels(channels)
        f.setsampwidth(2)
        f.setframerate(rate)
        f.writeframes(samples.astype("<i2").tobytes())
    return buffer.getvalue()


def wav_header(format_tag: int, bits: int, frames: bytes, rate: int = 16_000) -> bytes:
    block = bits // 8
    fmt = struct.pack("<HHIIHH", format_tag, 1, rate, rate * block, block, bits)
    return (
        b"RIFF" + struct.pack("<I", 4 + 8 + len(fmt) + 8 + len(frames)) + b"WAVE"
        + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(frames)) + frames
    )


SPEECH = wav(np.full(16_000, 1000))


@pytest.fixture
def serve(tmp_path):
    servers: list[DictationServer] = []

    def start(transcriber=None, glm=None, *, postprocess=True, timeout_s=2.0, model=None) -> DictationServer:
        if model is None:
            model = SharedModel()
            model.load(transcriber or FakeTranscriber())
        server = DictationServer(
            ServerConfig(bind="127.0.0.1", port=0), model=model, polisher=glm, polish_enabled=lambda: postprocess,
            timeout_s=timeout_s, token_path=tmp_path / "server_token.txt",
        )
        server.start()
        servers.append(server)
        return server

    yield start
    for server in servers:
        server.stop()


def url(server: DictationServer, path: str) -> str:
    return f"http://127.0.0.1:{server.port}{path}"


def auth(server: DictationServer, token: str | None) -> dict[str, str]:
    """The server's own token by default; "" sends no Authorization header."""
    return {} if token == "" else {"Authorization": f"Bearer {server.token if token is None else token}"}


def post(server: DictationServer, audio: bytes = SPEECH, token: str | None = None, **fields: str) -> httpx2.Response:
    return httpx2.post(
        url(server, "/v1/audio/transcriptions"),
        files={"file": ("speech.wav", audio, "audio/wav")},
        data=fields,
        headers=auth(server, token),
        timeout=10,
    )


def health(server: DictationServer, token: str | None = None) -> httpx2.Response:
    return httpx2.get(url(server, "/v1/health"), headers=auth(server, token), timeout=10)


@pytest.mark.parametrize("reason", ["end_turn", "refusal", "max_tokens"])
def test_server_uses_claude_for_mac_instructions_and_preserves_raw_fallback(serve, reason):
    seen = []

    def handler(request):
        if request.method == "OPTIONS":
            return httpx2.Response(200)
        seen.append(json.loads(request.content))
        return httpx2.Response(200, json={
            "content": [{"type": "text", "text": POLISHED}], "stop_reason": reason,
            "stop_details": {"category": "cyber"} if reason == "refusal" else None,
        })

    client = create_polisher(PostprocessConfig(provider="anthropic"), "test-key", transport=httpx2.MockTransport(handler))
    try:
        server = serve(glm=client, postprocess=False)
        response = post(server, postprocess="true", instructions="Translate to English.")
        assert response.status_code == 200
        assert response.json() == {
            "text": POLISHED if reason == "end_turn" else RAW, "raw": RAW, "polished": reason == "end_turn",
        }
        assert seen[0]["system"].endswith("\n\nTranslate to English.")
        assert seen[0]["model"] == "claude-haiku-5-5"
        assert post(server, postprocess="false", instructions="Be formal.").json()["polished"] is False
        assert len(seen) == 1
    finally:
        client.close()


def test_the_token_is_created_once_and_kept(serve, tmp_path):
    first = serve()
    token = (tmp_path / "server_token.txt").read_text(encoding="utf-8")
    assert first.token == token and len(token) >= 40
    assert serve().token == token


@pytest.mark.parametrize("token", ["", "wrong", "{token}x"])
def test_a_missing_or_wrong_token_gets_401_and_transcribes_nothing(serve, token):
    transcriber = FakeTranscriber()
    server = serve(transcriber)
    token = token.format(token=server.token)
    assert health(server, token).status_code == 401
    response = post(server, token=token)
    assert response.status_code == 401
    assert response.json() == {"error": {"message": "missing or wrong bearer token"}}
    assert transcriber.audio == []


@pytest.mark.parametrize(
    "audio",
    [b"not a wav at all", wav_header(1, 24, bytes(3 * 1600)), wav_header(3, 32, np.zeros(1600, "<f4").tobytes())],
    ids=["not-wav", "pcm24", "float32"],
)
def test_audio_that_is_not_16_bit_pcm_wav_gets_400_naming_the_format(serve, audio):
    transcriber = FakeTranscriber()
    response = post(serve(transcriber), audio)
    assert response.status_code == 400
    assert "16-bit PCM" in response.json()["error"]["message"]
    assert transcriber.audio == []


def test_a_wav_comes_back_transcribed_and_polished(serve):
    glm = FakeGlm()
    response = post(serve(glm=glm))
    assert (response.status_code, response.json()) == (200, {"text": POLISHED, "raw": RAW, "polished": True})
    assert glm.polished == [RAW]


def test_stereo_at_any_rate_reaches_the_model_as_mono_at_that_rate(serve):
    transcriber = FakeTranscriber()
    left, right = np.full(44_100, 3000), np.full(44_100, 1000)
    stereo = wav(np.column_stack([left, right]).ravel(), rate=44_100, channels=2)
    assert post(serve(transcriber), stereo, postprocess="false").status_code == 200
    (audio,) = transcriber.audio
    assert (audio.rate, audio.samples.shape, audio.samples.dtype) == (44_100, (44_100,), np.float32)
    assert np.allclose(audio.samples, 2000 / 32768)


def test_postprocess_false_skips_glm(serve):
    glm = FakeGlm()
    response = post(serve(glm=glm), postprocess="false")
    assert response.json() == {"text": RAW, "raw": RAW, "polished": False}
    assert glm.polished == []


def test_postprocess_defaults_to_the_tray_choice(serve):
    glm = FakeGlm()
    assert post(serve(glm=glm, postprocess=False)).json()["polished"] is False
    assert post(serve(glm=glm, postprocess=False), postprocess="true").json()["polished"] is True


def test_instructions_reach_glm(serve):
    glm = FakeGlm()
    server = serve(glm=glm)
    assert post(server, instructions=" Translate to English. ").json()["polished"] is True
    assert post(server).json()["polished"] is True
    assert glm.instructions == ["Translate to English.", ""]


def test_instructions_that_are_not_utf8_get_400(serve):
    glm = FakeGlm()
    server = serve(glm=glm)
    response = httpx2.post(
        url(server, "/v1/audio/transcriptions"),
        files={"file": ("speech.wav", SPEECH, "audio/wav"), "instructions": (None, b"\xff\xfe")},
        headers=auth(server, None), timeout=10,
    )
    assert response.status_code == 400 and "UTF-8" in response.json()["error"]["message"]
    assert glm.polished == []


@pytest.mark.parametrize(
    "glm", [FakeGlm(error=PolishError("HTTP 500")), FakeGlm(error=RuntimeError("boom")), FakeGlm(seconds=2.0)],
    ids=["polish-error", "crash", "slower-than-timeout"],
)
def test_a_glm_failure_or_timeout_returns_the_raw_text(serve, glm):
    t0 = time.perf_counter()
    response = post(serve(glm=glm, timeout_s=0.3))
    assert (response.status_code, response.json()) == (200, {"text": RAW, "raw": RAW, "polished": False})
    assert time.perf_counter() - t0 < 1.5


def test_empty_speech_returns_empty_text_without_glm(serve):
    glm = FakeGlm()
    response = post(serve(FakeTranscriber(text=""), glm))
    assert response.json() == {"text": "", "raw": "", "polished": False}
    assert glm.polished == []


def test_health_reports_postprocess_and_warms_glm_only_when_on(serve):
    on, off = FakeGlm(), FakeGlm()
    assert health(serve(glm=on)).json() == {"status": "ok", "postprocess": True}
    assert on.preconnected.wait(5)
    assert health(serve(glm=off, postprocess=False)).json() == {"status": "ok", "postprocess": False}
    assert health(serve(glm=None)).json() == {"status": "ok", "postprocess": False}
    time.sleep(0.1)
    assert not off.preconnected.is_set()


def test_503_while_the_model_loads(serve):
    response = post(serve(model=SharedModel()))
    assert response.status_code == 503


def test_413_above_the_size_cap_without_reading_the_body(serve):
    server = serve()
    connection = http.client.HTTPConnection("127.0.0.1", server.port, timeout=10)
    connection.putrequest("POST", "/v1/audio/transcriptions")
    connection.putheader("Authorization", f"Bearer {server.token}")
    connection.putheader("Content-Type", "multipart/form-data; boundary=x")
    connection.putheader("Content-Length", str(MAX_BODY_BYTES + 1))
    connection.endheaders()
    assert connection.getresponse().status == 413
    connection.close()


def test_a_server_request_during_a_local_transcription_waits_for_it(serve):
    transcriber = FakeTranscriber(seconds=0.5)
    model = SharedModel()
    model.load(transcriber)
    pasted = threading.Event()

    class Recorder:
        def start(self) -> None:
            pass

        def stop(self) -> Audio:
            return Audio(np.zeros(16_000, np.float32), 16_000)

        def discard(self) -> None:
            pass

    dictation = Dictation(
        Config(), recorder=Recorder(), transcriber=model, paste=lambda text: pasted.set(), cues=Cues(enabled=False)
    )
    server = serve(model=model)
    # The controller alone: Dictation.start() would also install the real keyboard hook.
    dictation._controller.start()
    try:
        dictation.post(HotkeyDown(0.0))
        dictation.post(HotkeyUp(2.0))
        assert transcriber.entered.wait(5)
        response = post(server, postprocess="false")
        assert pasted.wait(5)
    finally:
        dictation.stop()

    assert response.json()["raw"] == RAW
    (local_in, local_out), (server_in, server_out) = transcriber.spans
    assert local_out <= server_in, "the server transcription overlapped the local one"
