import Foundation
import Testing

@testable import DictateCore

@Test func connectionInfoFromTheWindowsTrayParses() {
    let token = "Zt3Hq0m8tY3a_x1VbQk4pD2nR7sW9uE5oI6lA0cJ-fG"
    let info = ServerConnection.parse("http://192.168.3.2:8765\r\n\(token)\r\n")
    #expect(info == ServerConnection(baseURL: URL(string: "http://192.168.3.2:8765")!, token: token))
    #expect(ServerConnection.parse("  192.168.3.2:8765/ \n\n \(token) ")?.baseURL.absoluteString == "http://192.168.3.2:8765")
    #expect(ServerConnection.parse("http://192.168.3.2:8765") == nil)
    #expect(ServerConnection.parse("not a url at all\n\(token)") == nil)
    #expect(ServerConnection.parse("ftp://192.168.3.2\n\(token)") == nil)
}

@Test func wavHeaderMatchesTheContract() {
    let data = Wav.encode([0, 1, -1, 2, 0.5], rate: 16_000)
    #expect(data.count == 44 + 10)
    func u32(_ offset: Int) -> UInt32 { data[offset..<offset + 4].withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) } }
    func u16(_ offset: Int) -> UInt16 { data[offset..<offset + 2].withUnsafeBytes { $0.loadUnaligned(as: UInt16.self) } }
    func i16(_ offset: Int) -> Int16 { Int16(bitPattern: u16(offset)) }
    #expect(String(decoding: data[0..<4], as: UTF8.self) == "RIFF")
    #expect(u32(4) == 36 + 10)
    #expect(String(decoding: data[8..<16], as: UTF8.self) == "WAVEfmt ")
    #expect([u32(16), UInt32(u16(20)), UInt32(u16(22)), u32(24), u32(28), UInt32(u16(32)), UInt32(u16(34))]
        == [16, 1, 1, 16_000, 32_000, 2, 16])
    #expect(String(decoding: data[36..<40], as: UTF8.self) == "data")
    #expect(u32(40) == 10)
    #expect([i16(44), i16(46), i16(48), i16(50), i16(52)] == [0, 32767, -32767, 32767, 16384])
}

@Test func multipartBodyMatchesTheContract() {
    let body = ServerClient.multipart(wav: Data("WAV".utf8), postprocess: false, boundary: "b")
    #expect(String(decoding: body, as: UTF8.self) == """
        --b\r
        Content-Disposition: form-data; name="file"; filename="dictation.wav"\r
        Content-Type: audio/wav\r
        \r
        WAV\r
        --b\r
        Content-Disposition: form-data; name="postprocess"\r
        \r
        false\r
        --b--\r

        """)
    let noFlag = String(decoding: ServerClient.multipart(wav: Data(), postprocess: nil, boundary: "b"), as: UTF8.self)
    #expect(!noFlag.contains("postprocess") && !noFlag.contains("instructions"))
    let rule = ServerClient.multipart(wav: Data(), postprocess: true, instructions: "Translate to English.", boundary: "b")
    #expect(String(decoding: rule, as: UTF8.self).hasSuffix("""
        --b\r
        Content-Disposition: form-data; name="instructions"\r
        \r
        Translate to English.\r
        --b--\r

        """))
}

private func server(_ handler: @escaping (StubProtocol.Seen) -> StubProtocol.Answer) -> (ServerClient, String) {
    let (host, configuration) = StubProtocol.register(handler)
    let connection = ServerConnection(baseURL: URL(string: "http://\(host):8765")!, token: "tok")
    return (ServerClient(connection: connection, configuration: configuration), host)
}

@Test func transcriptionRequestCarriesTokenLengthAndFields() async throws {
    let (client, host) = server { _ in .reply(200, json(["text": "Olá.", "raw": "olá", "polished": true])) }
    let reply = try await client.transcribe(wav: Wav.encode([0, 0], rate: 16_000), postprocess: true)
    #expect(reply == TranscriptionReply(text: "Olá.", raw: "olá", polished: true))
    let seen = try #require(StubProtocol.requests(to: host).first)
    #expect(seen.request.httpMethod == "POST")
    #expect(seen.request.url?.path == "/v1/audio/transcriptions")
    #expect(seen.request.value(forHTTPHeaderField: "Authorization") == "Bearer tok")
    #expect(seen.request.value(forHTTPHeaderField: "Content-Type")?.hasPrefix("multipart/form-data; boundary=parakeet-") == true)
    #expect(String(decoding: seen.body, as: UTF8.self).contains("name=\"postprocess\"\r\n\r\ntrue\r\n"))
}

@Test func healthReportsPostprocess() async throws {
    let (client, host) = server { _ in .reply(200, json(["status": "ok", "postprocess": true])) }
    #expect(try await client.health())
    #expect(StubProtocol.requests(to: host).first?.request.url?.path == "/v1/health")
}

@Test func serverErrorsMapToReasons() async {
    func failure(_ answer: StubProtocol.Answer) async -> ServerError? {
        do {
            _ = try await server { _ in answer }.0.transcribe(wav: Data(), postprocess: nil)
            return nil
        } catch {
            return error as? ServerError
        }
    }
    let message = json(["error": ["message": "the file is not a 16-bit PCM WAV"]])
    #expect(await failure(.reply(401, message)) == .unauthorized)
    #expect(await failure(.reply(503, message)) == .loading)
    #expect(await failure(.reply(400, message)) == .http(400, "the file is not a 16-bit PCM WAV"))
    #expect(await failure(.reply(200, Data("{}".utf8))) == .badReply)
    #expect(await failure(.fail(.timedOut)) == .unreachable("timed out"))
}

@Test func tonesMatchTheWindowsCues() {
    // Two 60 ms notes and two 20 ms gaps at 44.1 kHz, in 16-bit samples, after the 44-byte header.
    #expect(Tones.wav(.start).count == 44 + 2 * (2 * 2646 + 2 * 882))
    #expect(Tones.wav(.cancel).count == 44 + 2 * (2646 + 882))
}

@Test(arguments: [48_000.0, 44_100.0, 24_000.0])
func resamplingKeepsDurationAndPitch(rate: Double) throws {
    // One second of 440 Hz.
    let tone = (0..<Int(rate)).map { Float(sin(2 * .pi * 440 * Double($0) / rate)) }
    let out = try Resampler.resample(tone, from: rate)
    #expect(abs(out.count - 16_000) <= 32)
    // Count rising zero crossings away from the edges: about 440 per second.
    let middle = out[800..<15_200]
    let crossings = zip(middle, middle.dropFirst()).filter { $0 < 0 && $1 >= 0 }.count
    #expect(abs(Double(crossings) - 440 * 0.9) <= 3)
}
