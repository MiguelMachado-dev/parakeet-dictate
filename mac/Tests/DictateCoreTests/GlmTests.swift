import Foundation
import Testing

@testable import DictateCore

// A port of tests/test_polish.py.

private let raw = "um so I think we should uh push the release"

private func reply(_ content: String, finishReason: String = "stop") -> StubProtocol.Answer {
    .reply(200, json([
        "choices": [["message": ["role": "assistant", "content": content], "finish_reason": finishReason]],
        "usage": ["completion_tokens": 12, "completion_tokens_details": ["reasoning_tokens": 3]],
    ]))
}

private func client(_ handler: @escaping (StubProtocol.Seen) -> StubProtocol.Answer) -> (GlmClient, String) {
    let (host, configuration) = StubProtocol.register(handler)
    var config = GlmConfig()
    config.endpoint = URL(string: "https://\(host)/api/coding/paas/v4/chat/completions")!
    config.reasoningEffort = "high"
    config.extraInstructions = "Names: João."
    return (GlmClient(config: config, apiKey: "test-key", configuration: configuration), host)
}

private func polishError(_ handler: @escaping (StubProtocol.Seen) -> StubProtocol.Answer) async -> String {
    do {
        _ = try await client(handler).0.polish(raw)
        Issue.record("expected a PolishError")
        return ""
    } catch let error as PolishError {
        #expect(!error.description.contains(raw) && !error.description.contains("push the release"))
        return error.description
    } catch {
        Issue.record("expected a PolishError, got \(error)")
        return ""
    }
}

@Test func requestCarriesAuthThinkingEffortAndTheTaggedTranscript() async throws {
    let (glm, host) = client { _ in reply("I think we should push the release.") }
    #expect(try await glm.polish(raw) == "I think we should push the release.")
    let seen = StubProtocol.requests(to: host)
    try #require(seen.count == 1)
    let request = seen[0].request
    let body = try #require(try JSONSerialization.jsonObject(with: seen[0].body) as? [String: Any])
    #expect(request.httpMethod == "POST")
    #expect(request.url?.path == "/api/coding/paas/v4/chat/completions")
    #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer test-key")
    #expect(body["model"] as? String == "glm-5.3")
    #expect(body["thinking"] as? [String: String] == ["type": "enabled"])
    #expect(body["reasoning_effort"] as? String == "high")
    #expect(body["stream"] as? Bool == false)
    let messages = try #require(body["messages"] as? [[String: String]])
    #expect(messages.count == 2)
    #expect(messages[0]["role"] == "system")
    #expect(messages[0]["content"]?.hasSuffix("\n\nNames: João.") == true)
    #expect(messages[0]["content"]?.hasPrefix("You clean up raw speech-to-text dictation. The user message") == true)
    #expect(messages[1] == ["role": "user", "content": "<transcript>\(raw)</transcript>"])
}

@Test(arguments: [
    ("  I think so.\n", "I think so."),
    ("\"I think so.\"", "I think so."),
    ("\u{201C}I think so.\u{201D}", "I think so."),
    ("<transcript>I think so.</transcript>", "I think so."),
    ("<transcript>\"I think so.\"</transcript>", "I think so."),
    ("\"<transcript> I think so. </transcript>\"", "I think so."),
    ("He said \"stop\" twice.", "He said \"stop\" twice."),
])
func replyIsStrippedOfWhitespaceQuotesAndEchoedTags(content: String, expected: String) async throws {
    #expect(try await client { _ in reply(content) }.0.polish(raw) == expected)
}

@Test func timeoutIsAPolishError() async {
    #expect(await polishError { _ in .fail(.timedOut) } == "no reply within 8 s")
}

@Test func networkErrorIsAPolishError() async {
    #expect(await polishError { _ in .fail(.cannotConnectToHost) }.hasPrefix("URLError"))
}

@Test func httpErrorReportsStatusAndZaiCode() async {
    let rateLimited = json(["error": ["code": "1113", "message": "x"]])
    #expect(await polishError { _ in .reply(429, rateLimited) } == "HTTP 429, Z.ai error 1113")
    #expect(await polishError { _ in .reply(502, Data("bad gateway".utf8)) } == "HTTP 502")
}

@Test(arguments: [
    StubProtocol.Answer.reply(200, Data("not json".utf8)),
    .reply(200, json(["choices": []])),
    .reply(200, json(["choices": [["message": ["content": NSNull()], "finish_reason": "stop"]]])),
    reply("partial", finishReason: "length"),
    reply("", finishReason: "sensitive"),
])
func malformedOrUnfinishedReplyIsAPolishError(answer: StubProtocol.Answer) async {
    #expect(await !polishError { _ in answer }.isEmpty)
}

@Test(arguments: ["", "   ", "\"\"", "<transcript></transcript>"])
func emptyReplyIsRejected(content: String) async {
    #expect(await polishError { _ in reply(content) } == "empty reply")
}

@Test func replyMuchLongerThanTheTranscriptIsRejected() async throws {
    let limit = 2 * raw.count + 40
    let longest = String(repeating: "x", count: limit)
    #expect(try await client { _ in reply(longest) }.0.polish(raw) == longest)
    let tooLong = String(repeating: "x", count: limit + 1)
    #expect(await polishError { _ in reply(tooLong) }.hasPrefix("reply of \(limit + 1) chars"))
}

@Test func preconnectOpensAConnectionOnlyWhenNoneIsKeptAlive() async throws {
    let (glm, host) = client { seen in
        seen.request.httpMethod == "POST" ? reply("ok") : .reply(200, Data())
    }
    await glm.preconnect()
    await glm.preconnect()
    _ = try await glm.polish(raw)
    await glm.preconnect()
    #expect(StubProtocol.requests(to: host).map(\.request.httpMethod) == ["OPTIONS", "POST"])
}

@Test func preconnectFailureIsSilent() async {
    await client { _ in .fail(.cannotConnectToHost) }.0.preconnect()
}

@Test func instructionsFollowTheSystemPromptForOneDictationOnly() async throws {
    let (glm, host) = client { _ in reply("I think we should push the release.") }
    _ = try await glm.polish(raw, instructions: "  Translate to English.\n")
    _ = try await glm.polish(raw)
    let systems = try StubProtocol.requests(to: host).map { seen in
        let body = try #require(try JSONSerialization.jsonObject(with: seen.body) as? [String: Any])
        let messages = try #require(body["messages"] as? [[String: String]])
        return try #require(messages[0]["content"])
    }
    try #require(systems.count == 2)
    #expect(systems[0] == "\(systems[1])\n\n\(polishInstructionsPrompt)\n\nTranslate to English.")
    #expect(systems[1].hasSuffix("\n\nNames: João."))
}
