import AVFoundation
import DictateCore
import QuartzCore

/// Times consecutive steps of a call, for logging where a slow one spent its time.
struct Laps: CustomStringConvertible {
    private let started = ProcessInfo.processInfo.systemUptime
    private var last: Double
    private(set) var steps: [(name: String, ms: Int)] = []

    init() {
        last = started
    }

    mutating func lap(_ name: String) {
        let now = ProcessInfo.processInfo.systemUptime
        steps.append((name, Int(((now - last) * 1000).rounded())))
        last = now
    }

    var totalMs: Int {
        Int(((last - started) * 1000).rounded())
    }

    /// "312 ms (input 280, start 30)"
    var description: String {
        "\(totalMs) ms (" + steps.map { "\($0.name) \($0.ms)" }.joined(separator: ", ") + ")"
    }
}

struct RecorderError: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = description
    }
}

/// Records the default microphone at its own rate, like recorder.py, and hands back 16 kHz mono.
/// A new engine per recording picks up a headset connected since the last one, and keeps the
/// microphone off (no orange dot) between dictations. The audio thread and the overlay touch only what
/// the lock guards and the meter, which has its own lock.
final class Recorder: @unchecked Sendable {
    private var engine: AVAudioEngine?
    private let lock = NSLock()
    private var chunks: [[Float]] = []
    private var rate: Double = Resampler.modelRate
    private(set) var deviceName = ""
    /// Where the last start() and stop() spent their time. A Bluetooth headset switches profile in them.
    private(set) var startLaps = Laps()
    private(set) var stopLaps = Laps()
    /// Replaced on each start, so the overlay always reads levels of the current recording at the rate
    /// its stream actually runs. Kept after the recording stops: the overlay fades out from those levels.
    private var meter: LevelMeter?

    /// The current recording's levels and spectrum as of `now` (CACurrentMediaTime), from any thread.
    func levels(at now: Double) -> (levels: [Float], spectrum: [Float]) {
        guard let meter = lock.withLock({ meter }) else {
            return (
                [Float](repeating: 0, count: LevelMeter.levelHistory), [Float](repeating: 0, count: LevelMeter.spectrumBands)
            )
        }
        return meter.snapshot(at: now)
    }

    func start() throws {
        discard()
        var laps = Laps()
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            throw RecorderError("microphone access is not granted")
        }
        laps.lap("permission")
        let engine = AVAudioEngine()
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        laps.lap("input")
        guard format.sampleRate > 0, format.channelCount > 0 else {
            throw RecorderError("no microphone")
        }
        rate = format.sampleRate
        let meter = LevelMeter(rate: format.sampleRate)
        lock.withLock { self.meter = meter }
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
            guard let self, let channels = buffer.floatChannelData else {
                return
            }
            let samples = UnsafeBufferPointer(start: channels[0], count: Int(buffer.frameLength))
            meter.feed(samples, at: CACurrentMediaTime())
            let copy = Array(samples)
            self.lock.withLock { self.chunks.append(copy) }
        }
        engine.prepare()
        laps.lap("prepare")
        do {
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            throw RecorderError("microphone did not start: \(error.localizedDescription)")
        }
        laps.lap("start")
        self.engine = engine
        deviceName = AVCaptureDevice.default(for: .audio)?.localizedName ?? "the default input"
        laps.lap("name")
        startLaps = laps
    }

    /// The recording at 16 kHz mono.
    func stop() throws -> [Float] {
        guard engine != nil else {
            throw RecorderError("stop() without start()")
        }
        var laps = Laps()
        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        laps.lap("stop")
        // The last reference, so the engine and what it autoreleased go here rather than later on this thread.
        autoreleasepool { engine = nil }
        laps.lap("release")
        let (samples, buffers) = lock.withLock { () -> ([Float], Int) in
            defer { chunks = [] }
            return (chunks.flatMap { $0 }, chunks.count)
        }
        guard !samples.isEmpty else {
            throw RecorderError("no audio from \(deviceName)")
        }
        let resampled = try Resampler.resample(samples, from: rate)
        laps.lap("resample")
        stopLaps = laps
        Log.info(
            "\(deviceName) delivered audio every \(Int(Double(samples.count) / Double(buffers) / rate * 1000)) ms; "
                + "stopped in \(laps)")
        return resampled
    }

    func discard() {
        if engine != nil {
            engine?.inputNode.removeTap(onBus: 0)
            engine?.stop()
            autoreleasepool { engine = nil }
        }
        lock.withLock { chunks = [] }
    }
}
