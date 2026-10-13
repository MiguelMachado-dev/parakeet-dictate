import Foundation

// Post-processing with Claude through the Anthropic Messages API, with the prompt and reply checks of
// Polish.swift. The PC has no counterpart: it post-processes with GLM only.

public struct ClaudeConfig: Equatable, Sendable {
    public static let haiku = "claude-haiku-5-5"
    public static let sonnet = "claude-sonnet-5-5"

    public var model = ClaudeConfig.haiku
    public var endpoint = URL(string: "https://api.anthropic.com/v1/messages")!
    /// "low", "medium", "high", "xhigh", or "max". Higher effort thinks longer when thinking is on.
    public var effort = "low"
    /// Off is the fastest: the model writes the cleaned text without reasoning first. The API takes it
    /// only at effort "high" or below.
    public var thinking = false
    /// Seconds to wait for Claude before pasting the raw transcript.
    public var timeout: TimeInterval = 8
    /// Appended to the system prompt, for names and jargon to spell correctly.
    public var extraInstructions = ""

    public init() {}

    /// Each model turns thinking off its own way: Claude Sonnet 5.5 rejects "disabled" and takes
    /// "between_tools", which every other model rejects.
    var thinkingParameter: [String: String] {
        if thinking {
            return ["type": "adaptive"]
        }
        return ["type": model == Self.sonnet ? "between_tools" : "disabled"]
    }
}

public actor ClaudeClient: Polisher {
    static let apiVersion = "2023-06-01"
    /// Lets the API re-run a declined request on another model within the same call. Claude Sonnet 5.5
    /// has it; Claude Haiku 5.5 has no fallback model.
    static let fallbackBeta = "server-side-fallback-2026-07-01"

    private let config: ClaudeConfig
    private let apiKey: String
    private let systemPrompt: String
    private var pool: PooledSession

    public init(config: ClaudeConfig, apiKey: String, configuration: URLSessionConfiguration = .ephemeral) {
        self.config = config
        self.apiKey = apiKey
        self.systemPrompt = polishPrompt(extraInstructions: config.extraInstructions)
        self.pool = PooledSession(configuration: configuration)
    }

    public func preconnect() async {
        guard pool.renewIfIdle() else {
            return
        }
        var request = URLRequest(url: config.endpoint)
        request.httpMethod = "OPTIONS"
        request.timeoutInterval = config.timeout
        do {
            _ = try await pool.session.data(for: request)
            pool.touch()
        } catch {
            Log.info("Claude pre-connect failed: \(type(of: error))")
        }
    }

    /// `instructions` come from the user, for this dictation only.
    public func polish(_ raw: String, instructions: String = "") async throws -> String {
        let system = polishPrompt(systemPrompt, instructions: instructions)
        _ = pool.renewIfIdle()
        pool.touch()
        var request = URLRequest(url: config.endpoint)
        request.httpMethod = "POST"
        request.timeoutInterval = config.timeout
        request.setValue(apiKey, forHTTPHeaderField: "x-api-key")
        request.setValue(Self.apiVersion, forHTTPHeaderField: "anthropic-version")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        var body: [String: Any] = [
            "model": config.model,
            // Thinking counts toward this cap, so it is not sized to the reply. The timeout ends a long one first.
            "max_tokens": 16_000,
            "system": system,
            "messages": [["role": "user", "content": "<transcript>\(raw)</transcript>"]],
            "thinking": config.thinkingParameter,
            "output_config": ["effort": config.effort],
        ]
        if config.model == ClaudeConfig.sonnet {
            request.setValue(Self.fallbackBeta, forHTTPHeaderField: "anthropic-beta")
            body["fallbacks"] = "default"
        }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)

        let started = Date()
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await pool.session.data(for: request)
            pool.touch()
        } catch let error as URLError where error.code == .timedOut {
            throw PolishError("no reply within \(formatSeconds(config.timeout)) s")
        } catch let error as URLError where error.code == .cancelled {
            throw CancellationError()
        } catch let error as URLError {
            throw PolishError("URLError \(error.code.rawValue)")
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard status == 200 else {
            throw PolishError("HTTP \(status)\(anthropicError(data))")
        }
        guard let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let content = payload["content"] as? [[String: Any]]
        else {
            throw PolishError("unexpected reply shape")
        }
        let stopReason = payload["stop_reason"] as? String
        if stopReason == "refusal" {
            let category = (payload["stop_details"] as? [String: Any])?["category"] as? String
            throw PolishError("declined (category \(category ?? "none"))")
        }
        guard stopReason == "end_turn" else {
            throw PolishError("reply did not finish normally (stop_reason \(stopReason ?? "none"))")
        }
        // A reply can open with thinking blocks, empty unless asked for, so only its text blocks count.
        let reply = content.filter { $0["type"] as? String == "text" }.compactMap { $0["text"] as? String }.joined()
        let text = try cleanReply(raw: raw, reply: reply)
        let model = payload["model"] as? String ?? config.model
        let tokens = ((payload["usage"] as? [String: Any])?["output_tokens"] as? Int).map(String.init) ?? "unknown"
        Log.info(
            "\(model) replied in \(Int(Date().timeIntervalSince(started) * 1000)) ms with \(tokens) output tokens: "
                + "\(raw.count) -> \(text.count) chars")
        return text
    }
}

/// The error's type and message, which says why a setting was rejected, such as thinking off on a model
/// that always thinks. Validation messages name parameters, not the transcript.
private func anthropicError(_ data: Data) -> String {
    guard let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
        let error = payload["error"] as? [String: Any], let type = error["type"] as? String
    else {
        return ""
    }
    let message = (error["message"] as? String).map { ": \($0.prefix(200))" } ?? ""
    return ", \(type)\(message)"
}
