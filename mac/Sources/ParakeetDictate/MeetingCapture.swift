import AVFoundation

/// The microphone as a stream, for as long as a meeting lasts: Recorder's one-engine shape, but
/// each block goes to a handler instead of the whole meeting piling up in memory. A dictation
/// during a meeting opens its own engine on the same microphone, which macOS allows, and that
/// speech lands on the meeting's Me track as well.
final class MeetingMicCapture: @unchecked Sendable {
    /// Mono samples (channel 0, like Recorder) and the rate they run at, on the audio thread.
    typealias Handler = (_ samples: [Float], _ rate: Double) -> Void

    private var engine: AVAudioEngine?
    private(set) var deviceName = ""
    private(set) var rate: Double = 0

    func start(handler: @escaping Handler) throws {
        guard engine == nil else {
            throw RecorderError("the meeting microphone is already running")
        }
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
        let sampleRate = format.sampleRate
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
            guard let channels = buffer.floatChannelData else {
                return
            }
            let samples = Array(UnsafeBufferPointer(start: channels[0], count: Int(buffer.frameLength)))
            handler(samples, sampleRate)
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

    func stop() {
        guard engine != nil else {
            return
        }
        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        autoreleasepool { engine = nil }
    }

    deinit {
        stop()
    }
}
