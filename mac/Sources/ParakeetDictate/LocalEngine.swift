import DictateCore
import FluidAudio
import Foundation

/// Parakeet TDT 0.6B v3, the PC's model, converted to Core ML and run on the Neural Engine by FluidAudio.
actor LocalEngine {
    enum Status: Equatable {
        case notLoaded
        case downloading(Double)
        case loading
        case ready
        case failed(String)
    }

    static let minSeconds = 0.1

    private var manager: AsrManager?
    private var decoderLayers = 2
    private var loading: Task<Void, Never>?
    /// False after unload(), so a load still running when the user switched to the PC is dropped.
    private var wanted = false
    private(set) var status: Status = .notLoaded {
        didSet { continuation.yield(status) }
    }
    nonisolated let statusUpdates: AsyncStream<Status>
    private let continuation: AsyncStream<Status>.Continuation

    init() {
        (statusUpdates, continuation) = AsyncStream.makeStream()
    }

    /// Downloads the model on first use (about 470 MB), compiles it for this Mac, and warms it up.
    func load() async {
        wanted = true
        if manager != nil {
            return
        }
        if let loading {
            return await loading.value
        }
        let task = Task { await self.loadNow() }
        loading = task
        await task.value
        loading = nil
    }

    private func loadNow() async {
        let started = Date()
        let cached = AsrModels.modelsExist(at: AsrModels.defaultCacheDirectory(for: .v3), version: .v3)
        status = cached ? .loading : .downloading(0)
        do {
            let models = try await AsrModels.downloadAndLoad(version: .v3) { progress in
                Task { await self.downloadProgress(progress) }
            }
            let manager = AsrManager(config: .default)
            try await manager.loadModels(models)
            guard wanted else {
                await manager.cleanup()
                status = .notLoaded
                return
            }
            decoderLayers = models.version.decoderLayers
            self.manager = manager
            Log.info("local model loaded in \(Int(Date().timeIntervalSince(started) * 1000)) ms")
            await warmUp()
            status = .ready
        } catch {
            Log.error("local model failed to load: \(error)")
            status = .failed(error.localizedDescription)
        }
    }

    private func downloadProgress(_ progress: DownloadProgress) {
        guard case .downloading = status else {
            return
        }
        if case .compiling = progress.phase {
            status = .loading
        } else {
            status = .downloading(progress.fractionCompleted)
        }
    }

    /// The first inference on the Neural Engine is slow; run it before the user's first dictation.
    private func warmUp() async {
        var noise = SystemRandomNumberGenerator()
        let samples = (0..<Int(4 * Resampler.modelRate)).map { _ in Float.random(in: -0.01...0.01, using: &noise) }
        for seconds in [1.0, 4.0] {
            let started = Date()
            _ = try? await transcribe(Array(samples.prefix(Int(seconds * Resampler.modelRate))))
            Log.info("warm-up \(seconds) s: \(Int(Date().timeIntervalSince(started) * 1000)) ms")
        }
    }

    func unload() async {
        wanted = false
        await manager?.cleanup()
        manager = nil
        status = .notLoaded
    }

    /// 16 kHz mono samples to text. Returns "" for no speech, like transcriber.py.
    func transcribe(_ samples: [Float]) async throws -> String {
        guard let manager else {
            throw RecorderError(status == .notLoaded ? "the local model is not loaded" : "the local model is still loading")
        }
        if Double(samples.count) < Self.minSeconds * Resampler.modelRate {
            return ""
        }
        // FluidAudio refuses clips under 0.3 s; silence after the speech changes nothing.
        let minimum = ASRConstants.minimumRequiredSamples(forSampleRate: Int(Resampler.modelRate))
        let padded = samples.count < minimum ? samples + Array(repeating: 0, count: minimum - samples.count) : samples
        var state = TdtDecoderState.make(decoderLayers: decoderLayers)
        let text = try await manager.transcribe(padded, decoderState: &state).text
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return text.range(of: "\\w", options: .regularExpression) == nil ? "" : text
    }
}
