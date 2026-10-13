import Foundation

/// The PC's dictation server, as described in docs/mac-client.md.
public struct ServerConnection: Equatable, Sendable {
    public var baseURL: URL
    public var token: String

    public init(baseURL: URL, token: String) {
        self.baseURL = baseURL
        self.token = token
    }

    /// Reads what the Windows tray's "Copy Mac connection info" puts on the clipboard: the URL, then the token.
    public static func parse(_ text: String) -> ServerConnection? {
        let lines = text.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        guard lines.count == 2, let url = normalizedURL(lines[0]) else {
            return nil
        }
        return ServerConnection(baseURL: url, token: lines[1])
    }

    /// Accepts "192.168.3.2:8765" as well as "http://192.168.3.2:8765/".
    public static func normalizedURL(_ text: String) -> URL? {
        var text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.contains("://") {
            text = "http://" + text
        }
        while text.hasSuffix("/") {
            text.removeLast()
        }
        guard let url = URL(string: text), let scheme = url.scheme, ["http", "https"].contains(scheme),
            url.host != nil
        else {
            return nil
        }
        return url
    }
}

public struct TranscriptionReply: Decodable, Equatable, Sendable {
    public let text: String
    public let raw: String
    public let polished: Bool
}

public enum ServerError: Error, Equatable, CustomStringConvertible {
    case unreachable(String)
    case unauthorized
    case loading
    case http(Int, String)
    case badReply

    public var description: String {
        switch self {
        case .unreachable(let reason): return "PC unreachable: \(reason)"
        case .unauthorized: return "wrong token"
        case .loading: return "the PC is still loading its model"
        case .http(let status, let message): return "HTTP \(status): \(message)"
        case .badReply: return "unexpected reply from the PC"
        }
    }
}

public final class ServerClient: Sendable {
    public static let requestTimeout: TimeInterval = 60
    public static let healthTimeout: TimeInterval = 5

    public let connection: ServerConnection
    private let session: URLSession

    public init(connection: ServerConnection, configuration: URLSessionConfiguration = .ephemeral) {
        self.connection = connection
        configuration.urlCache = nil
        configuration.waitsForConnectivity = false
        self.session = URLSession(configuration: configuration)
    }

    /// Whether the PC will post-process. The call also makes the PC warm its connection to GLM.
    public func health() async throws -> Bool {
        var request = URLRequest(url: connection.baseURL.appendingPathComponent("v1/health"))
        request.timeoutInterval = Self.healthTimeout
        authorize(&request)
        let data = try await send(request, body: nil)
        struct Health: Decodable { let status: String; let postprocess: Bool }
        guard let health = try? JSONDecoder().decode(Health.self, from: data), health.status == "ok" else {
            throw ServerError.badReply
        }
        return health.postprocess
    }

    /// `instructions` need a PC updated for them; an older one ignores them.
    public func transcribe(wav: Data, postprocess: Bool?, instructions: String = "") async throws -> TranscriptionReply {
        let boundary = "parakeet-\(UUID().uuidString)"
        var request = URLRequest(url: connection.baseURL.appendingPathComponent("v1/audio/transcriptions"))
        request.httpMethod = "POST"
        request.timeoutInterval = Self.requestTimeout
        authorize(&request)
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        let body = Self.multipart(wav: wav, postprocess: postprocess, instructions: instructions, boundary: boundary)
        let data = try await send(request, body: body)
        guard let reply = try? JSONDecoder().decode(TranscriptionReply.self, from: data) else {
            throw ServerError.badReply
        }
        return reply
    }

    public static func multipart(wav: Data, postprocess: Bool?, instructions: String = "", boundary: String) -> Data {
        var body = Data()
        body.append(Data("--\(boundary)\r\n".utf8))
        body.append(Data("Content-Disposition: form-data; name=\"file\"; filename=\"dictation.wav\"\r\n".utf8))
        body.append(Data("Content-Type: audio/wav\r\n\r\n".utf8))
        body.append(wav)
        body.append(Data("\r\n".utf8))
        if let postprocess {
            body.append(Data("--\(boundary)\r\n".utf8))
            body.append(Data("Content-Disposition: form-data; name=\"postprocess\"\r\n\r\n".utf8))
            body.append(Data("\(postprocess)\r\n".utf8))
        }
        if !instructions.isEmpty {
            body.append(Data("--\(boundary)\r\n".utf8))
            body.append(Data("Content-Disposition: form-data; name=\"instructions\"\r\n\r\n".utf8))
            body.append(Data("\(instructions)\r\n".utf8))
        }
        body.append(Data("--\(boundary)--\r\n".utf8))
        return body
    }

    private func authorize(_ request: inout URLRequest) {
        request.setValue("Bearer \(connection.token)", forHTTPHeaderField: "Authorization")
    }

    private func send(_ request: URLRequest, body: Data?) async throws -> Data {
        let data: Data
        let response: URLResponse
        do {
            // upload(for:from:) sends the body whole with a Content-Length; the server rejects chunked uploads.
            if let body {
                (data, response) = try await session.upload(for: request, from: body)
            } else {
                (data, response) = try await session.data(for: request)
            }
        } catch let error as URLError where error.code == .cancelled {
            throw CancellationError()
        } catch let error as URLError {
            throw ServerError.unreachable(Self.describe(error))
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        switch status {
        case 200:
            return data
        case 401:
            throw ServerError.unauthorized
        case 503:
            throw ServerError.loading
        default:
            struct Failure: Decodable {
                struct Detail: Decodable { let message: String }
                let error: Detail
            }
            let message = (try? JSONDecoder().decode(Failure.self, from: data))?.error.message ?? "no message"
            throw ServerError.http(status, message)
        }
    }

    private static func describe(_ error: URLError) -> String {
        switch error.code {
        case .timedOut: return "timed out"
        case .cannotConnectToHost: return "connection refused"
        case .notConnectedToInternet: return "no network"
        case .appTransportSecurityRequiresSecureConnection: return "blocked by App Transport Security"
        default: return "URLError \(error.code.rawValue)"
        }
    }
}
