import json

import httpx2
import pytest

from parakeet_dictate.config import PostprocessConfig
from parakeet_dictate.polish import GlmClient, PolishError

RAW = "um so I think we should uh push the release"
CONFIG = PostprocessConfig(reasoning_effort="high", extra_instructions="Names: João.")


def reply(content, finish_reason="stop"):
    return httpx2.Response(200, json={
        "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": finish_reason}],
        "usage": {"completion_tokens": 12, "completion_tokens_details": {"reasoning_tokens": 3}},
    })


def client(handler, config=CONFIG):
    return GlmClient(config, "test-key", transport=httpx2.MockTransport(handler))


def polish_error(handler) -> str:
    with pytest.raises(PolishError) as caught:
        client(handler).polish(RAW)
    message = str(caught.value)
    assert RAW not in message and "push the release" not in message
    return message


def test_request_carries_auth_thinking_effort_and_the_tagged_transcript():
    seen = []

    def handler(request):
        seen.append(request)
        return reply("I think we should push the release.")

    assert client(handler).polish(RAW) == "I think we should push the release."
    (request,) = seen
    body = json.loads(request.content)
    assert request.method == "POST"
    assert str(request.url) == CONFIG.endpoint
    assert request.headers["Authorization"] == "Bearer test-key"
    assert body["model"] == "glm-5.3"
    assert body["thinking"] == {"type": "enabled"}
    assert body["reasoning_effort"] == "high"
    assert body["stream"] is False
    system, user = body["messages"]
    assert system["role"] == "system" and system["content"].endswith("\n\nNames: João.")
    assert user == {"role": "user", "content": f"<transcript>{RAW}</transcript>"}


@pytest.mark.parametrize("content, expected", [
    ("  I think so.\n", "I think so."),
    ('"I think so."', "I think so."),
    ("“I think so.”", "I think so."),
    ("<transcript>I think so.</transcript>", "I think so."),
    ('<transcript>"I think so."</transcript>', "I think so."),
    ('"<transcript> I think so. </transcript>"', "I think so."),
    ('He said "stop" twice.', 'He said "stop" twice.'),
])
def test_reply_is_stripped_of_whitespace_quotes_and_echoed_tags(content, expected):
    assert client(lambda request: reply(content)).polish(RAW) == expected


def test_timeout_is_a_polish_error():
    def handler(request):
        raise httpx2.ReadTimeout("timed out", request=request)

    assert polish_error(handler) == "no reply within 8 s"


def test_network_error_is_a_polish_error():
    def handler(request):
        raise httpx2.ConnectError("connection refused", request=request)

    assert "ConnectError" in polish_error(handler)


def test_http_error_reports_status_and_zai_code():
    assert polish_error(lambda r: httpx2.Response(429, json={"error": {"code": "1113", "message": "x"}})) == (
        "HTTP 429, Z.ai error 1113"
    )
    assert polish_error(lambda r: httpx2.Response(502, text="bad gateway")) == "HTTP 502"


@pytest.mark.parametrize("response", [
    httpx2.Response(200, text="not json"),
    httpx2.Response(200, json={"choices": []}),
    httpx2.Response(200, json={"choices": [{"message": {"content": None}, "finish_reason": "stop"}]}),
    reply("partial", finish_reason="length"),
    reply("", finish_reason="sensitive"),
])
def test_malformed_or_unfinished_reply_is_a_polish_error(response):
    polish_error(lambda request: response)


@pytest.mark.parametrize("content", ["", "   ", '""', "<transcript></transcript>"])
def test_empty_reply_is_rejected(content):
    assert polish_error(lambda request: reply(content)) == "empty reply"


def test_reply_much_longer_than_the_transcript_is_rejected():
    limit = 2 * len(RAW) + 40
    assert client(lambda request: reply("x" * limit)).polish(RAW) == "x" * limit
    assert polish_error(lambda request: reply("x" * (limit + 1))).startswith(f"reply of {limit + 1} chars")
