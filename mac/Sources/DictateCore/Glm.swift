import Foundation

// A port of src/parakeet_dictate/polish.py: the same request as the PC's, with the prompt and reply checks
// of Polish.swift.

public struct GlmConfig: Equatable, Sendable {
    public var model = "glm-5.3"
    public var endpoint = URL(string: "https://api.z.ai/api/coding/paas/v4/chat/completions")!
    /// "low", "high", or "max". GLM-5.3 always reasons; higher effort is slower.
    public var reasoningEffort = "low"
    /// Seconds to wait for GLM before pasting the raw transcript.
    public var timeout: TimeInterval = 8
    /// Appended to the system prompt, for names and jargon to spell correctly.
    public var extraInstructions = ""

    public init() {}
}

public actor GlmClient: Polisher {
    private let config: GlmConfig
    private let apiKey: String
    private let systemPrompt: String
    private var pool: PooledSession

    public init(config: GlmConfig, apiKey: String, configuration: URLSessionConfiguration = .ephemeral) {
        self.config = config
        self.apiKey = apiKey
        self.systemPrompt = polishPrompt(extraInstructions: config.extraInstructions)
        self.pool = PooledSession(configuration: configuration)
    }

    /// Opens a connection while the user is still speaking, unless one opened recently is still alive.
    /// A new connection spends about 0.9 s on DNS, TCP, and TLS, as measured from the PC.
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
            Log.info("GLM pre-connect failed: \(type(of: error))")
        }
    }

    public func polish(_ raw: String) async throws -> String {
        _ = pool.renewIfIdle()
        pool.touch()
        var request = URLRequest(url: config.endpoint)
        request.httpMethod = "POST"
        request.timeoutInterval = config.timeout
        request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body: [String: Any] = [
            "model": config.model,
            "messages": [
                ["role": "system", "content": systemPrompt],
                ["role": "user", "content": "<transcript>\(raw)</transcript>"],
            ],
            // GLM-5.3 cannot turn reasoning off; low effort keeps it to a few dozen tokens.
            "thinking": ["type": "enabled"],
            "reasoning_effort": config.reasoningEffort,
            "stream": false,
        ]
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
            throw PolishError("HTTP \(status)\(zaiErrorCode(data))")
        }
        guard let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let choices = payload["choices"] as? [[String: Any]], let choice = choices.first,
            let message = choice["message"] as? [String: Any]
        else {
            throw PolishError("unexpected reply shape")
        }
        let finishReason = choice["finish_reason"] as? String
        guard finishReason == "stop", let reply = message["content"] as? String else {
            throw PolishError("reply did not finish normally (finish_reason \(finishReason ?? "none"))")
        }
        let text = try cleanReply(raw: raw, reply: reply)
        let usage = payload["usage"] as? [String: Any]
        let details = usage?["completion_tokens_details"] as? [String: Any]
        let reasoning = (details?["reasoning_tokens"] as? Int).map(String.init) ?? "unknown"
        Log.info(
            "GLM replied in \(Int(Date().timeIntervalSince(started) * 1000)) ms with \(reasoning) reasoning tokens: "
                + "\(raw.count) -> \(text.count) chars")
        return text
    }
}

private func zaiErrorCode(_ data: Data) -> String {
    guard let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
        let error = payload["error"] as? [String: Any], let code = error["code"]
    else {
        return ""
    }
    return ", Z.ai error \(code)"
}
