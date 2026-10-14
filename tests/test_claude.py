import json
from dataclasses import replace

import httpx2
import pytest

from parakeet_dictate.config import PostprocessConfig
from parakeet_dictate.polish import ClaudeClient, INSTRUCTIONS_PROMPT, PolishError, create_polisher

RAW = "um so I think we should uh push the release"
CONFIG = PostprocessConfig(provider="anthropic", extra_instructions="Names: João.")


def reply(text="I think we should push the release.", reason="end_turn", **extra):
    return httpx2.Response(200, json={
        "content": [{"type": "thinking", "thinking": "private"}, {"type": "text", "text": text}],
        "stop_reason": reason, "usage": {"output_tokens": 12}, **extra,
    })


def client(handler, config=CONFIG):
    return create_polisher(config, "test-key", transport=httpx2.MockTransport(handler))


def polish_error(handler) -> str:
    with pytest.raises(PolishError) as caught:
        client(handler).polish(RAW)
    message = str(caught.value)
    assert RAW not in message and "push the release" not in message
    return message


def test_selected_provider_sends_the_messages_api_contract():
    seen = []

    def handler(request):
        seen.append(request)
        return reply()

    claude = client(handler)
    assert isinstance(claude, ClaudeClient)
    assert claude.polish(RAW) == "I think we should push the release."
    request, = seen
    body = json.loads(request.content)
    assert request.method == "POST" and str(request.url) == "https://api.anthropic.com/v1/messages"
    assert request.headers["x-api-key"] == "test-key"
    assert request.headers["anthropic-version"] == "2023-06-01"
    assert "authorization" not in request.headers and "anthropic-beta" not in request.headers
    assert body["model"] == "claude-haiku-5-5" and body["max_tokens"] == 16_000
    assert body["output_config"] == {"effort": "low"}
    assert body["thinking"] == {"type": "disabled"}
    assert body["system"].startswith("You clean up raw speech-to-text dictation.")
    assert body["system"].endswith("\n\nNames: João.")
    assert body["messages"] == [{"role": "user", "content": f"<transcript>{RAW}</transcript>"}]
    assert not {"temperature", "stream", "fallbacks"} & body.keys()


@pytest.mark.parametrize("model", ["claude-haiku-5-5", "claude-sonnet-5-5"])
@pytest.mark.parametrize("thinking", [True, False])
def test_thinking_and_fallback_are_specific_to_the_model(model, thinking):
    seen = []

    def handler(request):
        seen.append(request)
        return reply()

    config = replace(CONFIG, model=model, thinking=thinking, effort="max" if thinking else "high")
    client(handler, config).polish(RAW)
    request, = seen
    body = json.loads(request.content)
    sonnet = model == "claude-sonnet-5-5"
    assert body["thinking"] == {"type": "adaptive" if thinking else "between_tools" if sonnet else "disabled"}
    assert body["output_config"] == {"effort": "max" if thinking else "high"}
    if sonnet:
        assert request.headers["anthropic-beta"] == "server-side-fallback-2026-07-01"
        assert body["fallbacks"] == "default"
    else:
        assert "anthropic-beta" not in request.headers and "fallbacks" not in body


def test_instructions_apply_to_one_request_without_changing_the_base_prompt():
    seen = []

    def handler(request):
        seen.append(json.loads(request.content)["system"])
        return reply()

    claude = client(handler)
    claude.polish(RAW, "  Translate to English.\n")
    claude.polish(RAW)
    with_rule, without_rule = seen
    assert with_rule == f"{without_rule}\n\n{INSTRUCTIONS_PROMPT}\n\nTranslate to English."
    assert without_rule.endswith("\n\nNames: João.")


def test_only_text_blocks_are_joined_and_cleaned():
    response = reply(content=[
        {"type": "thinking", "thinking": "private"},
        {"type": "text", "text": '<transcript>" I think'},
        {"type": "text", "text": ' so. "</transcript>'},
    ])
    assert client(lambda r: response).polish(RAW) == "I think so."


def test_refusal_reports_its_category_and_discards_partial_text():
    assert polish_error(lambda r: reply(RAW, "refusal", stop_details={
        "category": "cyber", "explanation": RAW,
    })) == "declined (category cyber)"
    assert polish_error(lambda r: reply("", "refusal", stop_details=None)) == "declined (category none)"


@pytest.mark.parametrize("reason", ["max_tokens", "tool_use", "pause_turn", "stop_sequence", None])
def test_incomplete_reply_falls_back_instead_of_delivering_partial_text(reason):
    assert "did not finish normally" in polish_error(lambda r: reply("partial", reason))


@pytest.mark.parametrize("payload", [None, [], {}, {"content": None}, {"content": [None]},
    {"content": [{"type": "text", "text": None}], "stop_reason": "end_turn"}])
def test_malformed_reply_is_a_polish_error(payload):
    assert "unexpected reply shape" in polish_error(lambda r: httpx2.Response(200, json=payload))


def test_errors_keep_transcripts_keys_and_response_bodies_out_of_logs(caplog):
    message = polish_error(lambda r: httpx2.Response(400, json={
        "error": {"type": "invalid_request_error", "message": f"test-key: {RAW}"},
    }))
    assert message == "HTTP 400, invalid_request_error"
    assert polish_error(lambda r: httpx2.Response(502, text=RAW)) == "HTTP 502"
    assert "unexpected reply shape" in polish_error(lambda r: httpx2.Response(200, text=RAW))
    assert RAW not in caplog.text and "test-key" not in caplog.text


@pytest.mark.parametrize("error, expected", [
    (httpx2.ReadTimeout(RAW), "no reply within 8 s"),
    (httpx2.ConnectError(RAW), "ConnectError"),
])
def test_timeout_and_connection_errors_are_safe_to_log(error, expected):
    def handler(request):
        raise error

    assert polish_error(handler) == expected


@pytest.mark.parametrize("text", ["", "   ", '""', "<transcript></transcript>", "x" * (2 * len(RAW) + 41)])
def test_empty_or_overlong_text_is_rejected(text):
    polish_error(lambda r: reply(text))


def test_preconnect_reuses_a_recent_connection_and_renews_after_fifteen_seconds(monkeypatch):
    now, seen = [100.0], []
    monkeypatch.setattr("parakeet_dictate.polish.time.monotonic", lambda: now[0])

    def handler(request):
        seen.append(request.method)
        return reply() if request.method == "POST" else httpx2.Response(200)

    claude = client(handler)
    claude.preconnect()
    claude.preconnect()
    claude.polish(RAW)
    now[0] += 14
    claude.preconnect()
    now[0] += 1
    claude.preconnect()
    assert seen == ["OPTIONS", "POST", "OPTIONS"]


def test_preconnect_failure_does_not_fail_a_recording():
    def handler(request):
        raise httpx2.ConnectError("offline")

    client(handler).preconnect()
