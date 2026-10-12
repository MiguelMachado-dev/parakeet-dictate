import Foundation

/// Answers URLSession requests in tests. Each test registers its own handler under a fresh host,
/// so suites can run in parallel.
final class StubProtocol: URLProtocol {
    struct Seen {
        let request: URLRequest
        let body: Data
    }

    enum Answer {
        case reply(Int, Data)
        case fail(URLError.Code)
    }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var handlers: [String: (Seen) -> Answer] = [:]
    nonisolated(unsafe) private static var seen: [String: [Seen]] = [:]

    /// A host name and a session configuration that routes it to `handler`.
    static func register(_ handler: @escaping (Seen) -> Answer) -> (host: String, configuration: URLSessionConfiguration) {
        let host = "stub-\(UUID().uuidString.lowercased()).test"
        lock.withLock { handlers[host] = handler }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubProtocol.self]
        return (host, configuration)
    }

    static func requests(to host: String) -> [Seen] {
        lock.withLock { seen[host] ?? [] }
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let host = request.url?.host ?? ""
        let entry = Seen(request: request, body: Self.body(of: request))
        let handler = Self.lock.withLock { () -> ((Seen) -> Answer)? in
            Self.seen[host, default: []].append(entry)
            return Self.handlers[host]
        }
        switch handler?(entry) ?? .fail(.cannotFindHost) {
        case .reply(let status, let data):
            let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1", headerFields: nil)!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        case .fail(let code):
            client?.urlProtocol(self, didFailWithError: URLError(code))
        }
    }

    override func stopLoading() {}

    private static func body(of request: URLRequest) -> Data {
        if let body = request.httpBody {
            return body
        }
        guard let stream = request.httpBodyStream else {
            return Data()
        }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 65_536)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count <= 0 { break }
            data.append(buffer, count: count)
        }
        return data
    }
}

func json(_ object: Any) -> Data {
    try! JSONSerialization.data(withJSONObject: object)
}
