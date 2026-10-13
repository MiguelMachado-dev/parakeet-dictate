import Foundation
import Testing

@testable import DictateCore

private let raw = "um so I think we should uh push the release"

private func reply(_ text: String, stopReason: String = "end_turn") -> StubProtocol.Answer {
    .reply(200, json([
        "model": "claude-haiku-5-5",
        "content": [
            ["type": "thinking", "thinking": "", "signature": "sig"],
            ["type": "text", "text": text],
        ],
        "stop_reason": stopReason,
        "usage": ["input_tokens": 240, "output_tokens": 12],
    ]))
}

private func client(
    configuring configure: (inout ClaudeConfig) -> Void = { _ in },
    _ handler: @escaping (StubProtocol.Seen) -> StubProtocol.Answer
) -> (ClaudeClient, String) {
    let (host, configuration) = StubProtocol.register(handler)
    var config = ClaudeConfig()
    config.endpoint = URL(string: "https://\(host)/v1/messages")!
    config.extraInstructions = "Names: João."
    configure(&config)
    return (ClaudeClient(config: config, apiKey: "test-key", configuration: configuration), host)
}

private func sentBody(_ configure: (inout ClaudeConfig) -> Void) async throws -> ([String: Any], URLRequest) {
    let (claude, host) = client(configuring: configure) { _ in reply("I think we should push the release.") }
    _ = try await claude.polish(raw)
    let seen = try #require(StubProtocol.requests(to: host).first)
    let body = try #require(try JSONSerialization.jsonObject(with: seen.body) as? [String: Any])
    return (body, seen.request)
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

@Test func requestCarriesKeyVersionPromptAndTheTaggedTranscript() async throws {
    let (claude, host) = client { _ in reply("I think we should push the release.") }
    #expect(try await claude.polish(raw) == "I think we should push the release.")
    let seen = StubProtocol.requests(to: host)
    try #require(seen.count == 1)
    let request = seen[0].request
    let body = try #require(try JSONSerialization.jsonObject(with: seen[0].body) as? [String: Any])
    #expect(request.httpMethod == "POST")
    #expect(request.url?.path == "/v1/messages")
    #expect(request.value(forHTTPHeaderField: "x-api-key") == "test-key")
    #expect(request.value(forHTTPHeaderField: "anthropic-version") == "2023-06-01")
    #expect(request.value(forHTTPHeaderField: "Authorization") == nil)
    #expect(body["model"] as? String == "claude-haiku-5-5")
    #expect(body["max_tokens"] as? Int == 16_000)
    #expect(body["output_config"] as? [String: String] == ["effort": "low"])
    #expect(body["temperature"] == nil && body["stream"] == nil)
    let system = try #require(body["system"] as? String)
    #expect(system.hasPrefix("You clean up raw speech-to-text dictation. The user message"))
    #expect(system.hasSuffix("\n\nNames: João."))
    #expect(body["messages"] as? [[String: String]] == [["role": "user", "content": "<transcript>\(raw)</transcript>"]])
}

@Test func claudeInstructionsFollowTheSystemPromptForOneDictationOnly() async throws {
    let (claude, host) = client { _ in reply("I think we should push the release.") }
    _ = try await claude.polish(raw, instructions: "  Translate to English.\n")
    _ = try await claude.polish(raw)
    let systems = try StubProtocol.requests(to: host).map { seen in
        let body = try #require(try JSONSerialization.jsonObject(with: seen.body) as? [String: Any])
        return try #require(body["system"] as? String)
    }
    try #require(systems.count == 2)
    #expect(systems[0] == "\(systems[1])\n\n\(polishInstructionsPrompt)\n\nTranslate to English.")
    #expect(systems[1].hasSuffix("\n\nNames: João."))
}

@Test func thinkingOffIsDisabledExceptOnSonnet() async throws {
    #expect(try await sentBody { _ in }.0["thinking"] as? [String: String] == ["type": "disabled"])
    let sonnet = try await sentBody { $0.model = ClaudeConfig.sonnet }.0
    #expect(sonnet["thinking"] as? [String: String] == ["type": "between_tools"])
    let typed = try await sentBody { $0.model = "claude-opus-5-5" }.0
    #expect(typed["thinking"] as? [String: String] == ["type": "disabled"])
}

@Test func thinkingOnIsAdaptiveWithTheChosenEffort() async throws {
    for model in [ClaudeConfig.haiku, ClaudeConfig.sonnet] {
        let body = try await sentBody {
            $0.model = model
            $0.thinking = true
            $0.effort = "xhigh"
        }.0
        #expect(body["thinking"] as? [String: String] == ["type": "adaptive"])
        #expect(body["output_config"] as? [String: String] == ["effort": "xhigh"])
    }
}

@Test func onlySonnetAsksForTheServerSideFallback() async throws {
    let (haiku, haikuRequest) = try await sentBody { _ in }
    #expect(haiku["fallbacks"] == nil)
    #expect(haikuRequest.value(forHTTPHeaderField: "anthropic-beta") == nil)
    let (sonnet, sonnetRequest) = try await sentBody { $0.model = ClaudeConfig.sonnet }
    #expect(sonnet["fallbacks"] as? String == "default")
    #expect(sonnetRequest.value(forHTTPHeaderField: "anthropic-beta") == "server-side-fallback-2026-07-01")
}

@Test func onlyTextBlocksMakeTheReply() async throws {
    let answer = StubProtocol.Answer.reply(200, json([
        "content": [
            ["type": "thinking", "thinking": "The speaker said um.", "signature": "sig"],
            ["type": "text", "text": "I think we should "],
            ["type": "text", "text": "push the release."],
        ],
        "stop_reason": "end_turn",
    ]))
    #expect(try await client { _ in answer }.0.polish(raw) == "I think we should push the release.")
}

@Test func replyIsStrippedOfQuotesAndEchoedTags() async throws {
    #expect(try await client { _ in reply("<transcript>\"I think so.\"</transcript>") }.0.polish(raw) == "I think so.")
}

@Test func refusalIsAPolishErrorWithItsCategory() async {
    let declined = StubProtocol.Answer.reply(200, json([
        "content": [],
        "stop_reason": "refusal",
        "stop_details": ["type": "refusal", "category": "cyber", "explanation": "x"],
    ]))
    #expect(await polishError { _ in declined } == "declined (category cyber)")
}

@Test func httpErrorReportsStatusTypeAndMessage() async {
    let rejected = json([
        "type": "error",
        "error": ["type": "invalid_request_error", "message": "\"thinking.type.disabled\" is not supported for this model."],
    ])
    #expect(await polishError { _ in .reply(400, rejected) }
        == "HTTP 400, invalid_request_error: \"thinking.type.disabled\" is not supported for this model.")
    #expect(await polishError { _ in .reply(529, Data("overloaded".utf8)) } == "HTTP 529")
}

@Test func claudeTimeoutIsAPolishError() async {
    #expect(await polishError { _ in .fail(.timedOut) } == "no reply within 8 s")
}

@Test(arguments: [
    StubProtocol.Answer.reply(200, Data("not json".utf8)),
    .reply(200, json(["stop_reason": "end_turn"])),
    reply("partial", stopReason: "max_tokens"),
    reply("", stopReason: "end_turn"),
    .reply(200, json(["content": [["type": "thinking", "thinking": ""]], "stop_reason": "end_turn"])),
])
func malformedUnfinishedOrEmptyReplyIsAPolishError(answer: StubProtocol.Answer) async {
    #expect(await !polishError { _ in answer }.isEmpty)
}

@Test func claudePreconnectOpensAConnectionOnlyWhenNoneIsKeptAlive() async throws {
    let (claude, host) = client { seen in
        seen.request.httpMethod == "POST" ? reply("ok") : .reply(200, Data())
    }
    await claude.preconnect()
    await claude.preconnect()
    _ = try await claude.polish(raw)
    await claude.preconnect()
    #expect(StubProtocol.requests(to: host).map(\.request.httpMethod) == ["OPTIONS", "POST"])
}
