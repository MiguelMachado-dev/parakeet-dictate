import AVFoundation
import DictateCore

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
    /// Replaced on each start, so the overlay always reads levels of the current recording at the rate
    /// its stream actually runs. Kept after the recording stops: the overlay fades out from those levels.
    private var meter: LevelMeter?

    /// The current recording's levels and spectrum, from any thread.
    func levels() -> (levels: [Float], spectrum: [Float]) {
        guard let meter = lock.withLock({ meter }) else {
            return (
                [Float](repeating: 0, count: LevelMeter.levelHistory), [Float](repeating: 0, count: LevelMeter.spectrumBands)
            )
        }
        return meter.snapshot()
    }

    func start() throws {
        discard()
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            throw RecorderError("microphone access is not granted")
        }
        let engine = AVAudioEngine()
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
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
            meter.feed(samples)
            let copy = Array(samples)
            self.lock.withLock { self.chunks.append(copy) }
        }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            throw RecorderError("microphone did not start: \(error.localizedDescription)")
        }
        self.engine = engine
        deviceName = AVCaptureDevice.default(for: .audio)?.localizedName ?? "the default input"
    }

    /// The recording at 16 kHz mono.
    func stop() throws -> [Float] {
        guard let engine else {
            throw RecorderError("stop() without start()")
        }
        self.engine = nil
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        let samples = lock.withLock { () -> [Float] in
            defer { chunks = [] }
            return chunks.flatMap { $0 }
        }
        guard !samples.isEmpty else {
            throw RecorderError("no audio from \(deviceName)")
        }
        return try Resampler.resample(samples, from: rate)
    }

    func discard() {
        if let engine {
            engine.inputNode.removeTap(onBus: 0)
            engine.stop()
            self.engine = nil
        }
        lock.withLock { chunks = [] }
    }
}
