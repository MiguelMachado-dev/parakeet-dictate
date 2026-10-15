import AVFoundation
import AppKit
import DictateCore

// `ParakeetDictate --check-local a.wav b.wav`, `--check-server a.wav`, and `--check-polish "some text"` exercise
// each engine from the terminal with the app's own code and settings, without the menu bar app.
// `--check-polish` post-processes with the provider chosen in Settings; `--check-glm` is its old name.
// `--check-media` pauses and resumes what is playing, the way a dictation does.
// `--check-recorder [rounds] [pause]` records like a dictation and times where the microphone's start and stop spend
// their time, and how long the main thread stays busy after them. It needs the app's Microphone permission,
// so launch the bundle: open -W -n --stdout /dev/stdout "mac/build/Parakeet Dictate.app" --args --check-recorder
// PARAKEET_SERVER="<url> <token>" points --check-server somewhere other than the PC in Settings.
// PARAKEET_INSTRUCTIONS="Translate to English." gives --check-polish and --check-server an app rule's instructions.
let arguments = Array(CommandLine.arguments.dropFirst())
let instructions = ProcessInfo.processInfo.environment["PARAKEET_INSTRUCTIONS"] ?? ""

func samples(_ path: String) throws -> [Float] {
    let file = try AVAudioFile(forReading: URL(fileURLWithPath: path))
    guard let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length)) else {
        throw RecorderError("cannot read \(path)")
    }
    try file.read(into: buffer)
    let mono = Array(UnsafeBufferPointer(start: buffer.floatChannelData![0], count: Int(buffer.frameLength)))
    return try Resampler.resample(mono, from: file.processingFormat.sampleRate)
}

func check(_ name: String, _ body: () async throws -> Void) async -> Never {
    do {
        try await body()
        exit(0)
    } catch {
        print("\(name) failed: \(error)")
        exit(1)
    }
}

/// Pings the main queue every 5 ms from another thread, to find when something kept it busy.
final class MainLag: @unchecked Sendable {
    private let lock = NSLock()
    private var pings: [(sent: Double, served: Double)] = []

    init() {
        Thread.detachNewThread { [self] in
            while true {
                let sent = ProcessInfo.processInfo.systemUptime
                let served = DispatchSemaphore(value: 0)
                DispatchQueue.main.async {
                    let now = ProcessInfo.processInfo.systemUptime
                    self.lock.withLock { self.pings.append((sent, now)) }
                    served.signal()
                }
                served.wait()
                Thread.sleep(forTimeInterval: 0.005)
            }
        }
    }

    /// The longest a ping waited for the main queue between `from` and `to`, counting from `from` at the earliest.
    func worstMs(from: Double, to: Double) -> Int {
        let worst = lock.withLock {
            pings.filter { $0.served >= from && $0.served <= to }.map { $0.served - max($0.sent, from) }.max() ?? 0
        }
        return Int((worst * 1000).rounded())
    }
}

func uptime() -> Double {
    ProcessInfo.processInfo.systemUptime
}

func elapsed(since start: Date) -> String {
    "\(Int(Date().timeIntervalSince(start) * 1000)) ms"
}

switch arguments.first {
case "--check-local":
    await check("local") {
        let engine = LocalEngine()
        let started = Date()
        await engine.load()
        print("loaded in \(elapsed(since: started)): \(await engine.status)")
        for path in arguments.dropFirst() {
            let audio = try samples(path)
            let t = Date()
            let text = try await engine.transcribe(audio)
            print("\(path) (\(String(format: "%.1f", Double(audio.count) / 16_000)) s) in \(elapsed(since: t)): \(text)")
        }
    }
case "--check-server":
    await check("server") {
        let override = ProcessInfo.processInfo.environment["PARAKEET_SERVER"]
            .flatMap { ServerConnection.parse($0.replacingOccurrences(of: " ", with: "\n")) }
        guard let connection = override ?? Settings().serverConnection else {
            throw ServerError.unreachable("no PC address and token in Settings")
        }
        let client = ServerClient(connection: connection)
        print("health: postprocess \(try await client.health())")
        for path in arguments.dropFirst() {
            let t = Date()
            let wav = Wav.encode(try samples(path), rate: 16_000)
            for postprocess in [nil, true, false] as [Bool?] {
                let reply = try await client.transcribe(wav: wav, postprocess: postprocess, instructions: instructions)
                print("\(path) postprocess \(postprocess.map(String.init) ?? "omitted") in \(elapsed(since: t)): "
                    + "polished \(reply.polished): \(reply.text)")
            }
        }
    }
case "--check-polish", "--check-glm":
    let settings = Settings()
    await check(settings.polishProvider.modelName) {
        guard let polisher = settings.makePolisher() else {
            throw PolishError("no \(settings.polishProvider.keyName) API key in Settings")
        }
        if settings.polishProvider == .anthropic {
            let config = settings.claudeConfig
            print("\(config.model), effort \(config.effort), thinking \(config.thinking ? "on" : "off")")
        }
        let t = Date()
        await polisher.preconnect()
        print("pre-connect in \(elapsed(since: t))")
        let p = Date()
        let text = try await polisher.polish(arguments.dropFirst().joined(separator: " "), instructions: instructions)
        print("polished in \(elapsed(since: p)): \(text)")
    }
case "--check-media":
    await check("media") {
        let media = MediaPause()
        let wait = { (seconds: Double) in try await Task.sleep(for: .seconds(seconds)) }
        print("playing: \(MediaPause.playingApp() ?? "nothing")")
        media.recordingStarted()
        try await wait(0.1)
        media.recordingStopped()
        try await wait(0.5)
        print("after a 0.1 s shortcut: paused \(media.paused), playing: \(MediaPause.playingApp() ?? "nothing")")
        media.recordingStarted()
        try await wait(1.5)
        print("after 1.5 s of recording: paused \(media.paused), playing: \(MediaPause.playingApp() ?? "nothing")")
        media.recordingStopped()
        try await wait(1)
        print("1 s after it stopped: playing: \(MediaPause.playingApp() ?? "nothing")")
    }
case "--check-recorder":
    // Each round records 2 s like a hands-free dictation, transcribes it on this Mac from a task on the main
    // thread, as the app does, then pauses (5 s by default) so a Bluetooth headset switches back. Odd rounds play the start
    // and stop sounds as a dictation does; even rounds play none, so the two compare in one session.
    await check("recorder") {
        let rounds = arguments.dropFirst().first.flatMap(Int.init) ?? 6
        let pause = arguments.dropFirst(2).first.flatMap(Double.init) ?? 5
        let engine = LocalEngine()
        await engine.load()
        guard await engine.status == .ready else {
            throw RecorderError("the local model is \(await engine.status)")
        }
        let recorder = Recorder()
        let cues = Cues()
        let lag = MainLag()
        let wait = { (seconds: Double) in try await Task.sleep(for: .seconds(seconds)) }
        let soundMs = { (cue: Cue) -> Int in
            let t = uptime()
            cues.play(cue)
            return Int((uptime() - t) * 1000)
        }
        try await wait(1)
        for round in 1...rounds {
            let sounds = round % 2 == 1
            try recorder.start()
            let startSound = sounds ? "\(soundMs(.start)) ms" : "none"
            var t = uptime()
            try await wait(2)
            let afterStart = lag.worstMs(from: t, to: uptime())
            print("round \(round), \(recorder.deviceName), sounds \(sounds ? "on" : "off")")
            print("  start \(recorder.startLaps); start sound \(startSound); main busy up to \(afterStart) ms after")
            let samples = try recorder.stop()
            let stopped = uptime()
            let transcription = Task { @MainActor in
                let waited = uptime() - stopped
                let text = try await engine.transcribe(samples)
                return (waited, uptime() - stopped, text.count)
            }
            let stopSound = sounds ? "\(soundMs(.stop)) ms" : "none"
            t = uptime()
            let (waited, total, chars) = try await transcription.value
            try await wait(2)
            let afterStop = lag.worstMs(from: t, to: uptime())
            print("  stop \(recorder.stopLaps), \(String(format: "%.1f", Double(samples.count) / Resampler.modelRate)) s; "
                + "stop sound \(stopSound); main busy up to \(afterStop) ms after")
            print("  transcribed \(chars) chars in \(Int(total * 1000)) ms, started after \(Int(waited * 1000)) ms")
            try await wait(pause)
        }
    }
default:
    Log.startFile()
    let app = NSApplication.shared
    let delegate = AppDelegate()
    app.delegate = delegate
    app.setActivationPolicy(.accessory)
    app.run()
}
