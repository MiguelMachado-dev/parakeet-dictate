import DictateCore
import Foundation

struct MeetingError: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = description
    }
}

/// One meeting's Markdown in ~/Documents/Parakeet/Meetings, appended segment by segment as
/// they are transcribed, so a crash or a flat battery keeps everything heard so far.
final class MeetingFile: @unchecked Sendable {
    let url: URL
    private let handle: FileHandle
    private let lock = NSLock()
    private var closed = false

    static var folder: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Parakeet", isDirectory: true)
            .appendingPathComponent("Meetings", isDirectory: true)
    }

    static func create(startedAt: Date, microphone: String) throws -> MeetingFile {
        let manager = FileManager.default
        try manager.createDirectory(at: folder, withIntermediateDirectories: true)
        let name = MeetingTranscript.fileName(startedAt: startedAt)
        var url = folder.appendingPathComponent(name)
        var counter = 2
        while manager.fileExists(atPath: url.path) {
            url = folder.appendingPathComponent(
                name.replacingOccurrences(of: " Meeting.md", with: " Meeting \(counter).md"))
            counter += 1
        }
        guard manager.createFile(atPath: url.path, contents: nil) else {
            throw MeetingError("could not create \(url.lastPathComponent) in the Meetings folder")
        }
        let file = try MeetingFile(url: url)
        file.append(MeetingTranscript.header(startedAt: startedAt, microphone: microphone))
        return file
    }

    private init(url: URL) throws {
        self.url = url
        handle = try FileHandle(forWritingTo: url)
    }

    func append(_ text: String) {
        lock.withLock {
            if !closed {
                handle.write(Data(text.utf8))
            }
        }
    }

    func append(line: String) {
        append(line + "\n\n")
    }

    func finish(_ footer: String) {
        lock.withLock {
            if !closed {
                handle.write(Data(footer.utf8))
                closed = true
                try? handle.close()
            }
        }
    }
}

/// Turns both tracks' sample blocks into transcription jobs: one SpeechSegmenter per track on
/// a serial queue, each finished segment resampled once to the model's rate. Jobs come out in
/// the order their segments end, each carrying its start, so the file reads in meeting order.
/// The two segmenters are created by their first blocks, at the rates the captures actually
/// run at, and their timelines differ by however long the second capture took to start.
private final class MeetingPipeline: @unchecked Sendable {
    struct Job {
        let speaker: MeetingSpeaker
        let start: Double
        let samples: [Float]
    }

    private let queue = DispatchQueue(label: "parakeet-dictate.meeting-pipeline")
    private let continuation: AsyncStream<Job>.Continuation
    private var segmenters: [MeetingSpeaker: SpeechSegmenter] = [:]
    private var peaks: [MeetingSpeaker: Float] = [:]
    private var finished = false

    init(continuation: AsyncStream<Job>.Continuation) {
        self.continuation = continuation
    }

    func feed(_ samples: [Float], rate: Double, speaker: MeetingSpeaker) {
        queue.async { [self] in
            guard !finished else {
                return
            }
            peaks[speaker] = max(peaks[speaker] ?? 0, samples.reduce(Float(0)) { max($0, abs($1)) })
            let segmenter = segmenter(for: speaker, rate: rate)
            for segment in segmenter.feed(samples) {
                emit(segment, from: segmenter, speaker: speaker)
            }
        }
    }

    /// Flushes both segmenters and ends the job stream. Returns each track's loudest sample,
    /// so the log can say when a track heard nothing at all.
    func finish() -> [MeetingSpeaker: Float] {
        queue.sync {
            finished = true
            for speaker in [MeetingSpeaker.me, .others] {
                guard let segmenter = segmenters[speaker] else {
                    continue
                }
                for segment in segmenter.flush() {
                    emit(segment, from: segmenter, speaker: speaker)
                }
            }
            continuation.finish()
            return peaks
        }
    }

    private func segmenter(for speaker: MeetingSpeaker, rate: Double) -> SpeechSegmenter {
        if let existing = segmenters[speaker] {
            return existing
        }
        let created = SpeechSegmenter(rate: rate)
        segmenters[speaker] = created
        return created
    }

    private func emit(_ segment: SpeechSegmenter.Segment, from segmenter: SpeechSegmenter, speaker: MeetingSpeaker) {
        do {
            let samples = try Resampler.resample(segment.samples, from: segmenter.rate)
            continuation.yield(Job(speaker: speaker, start: segment.start, samples: samples))
        } catch {
            Log.error("a meeting segment could not be resampled: \(error)")
        }
    }
}

/// A meeting being transcribed: the microphone and the system audio captured side by side,
/// cut into utterances, transcribed on this Mac as they end, and written to one Markdown
/// file. Independent of the dictation reducer: dictations keep working during a meeting and
/// share the same local model. Meetings never go to the PC, whose contract is one clip per
/// request, so start() refuses while the PC is the chosen engine.
@MainActor
final class MeetingController {
    enum State: Equatable {
        case idle
        case recording
        case finishing
    }

    private(set) var state: State = .idle {
        didSet {
            if state != oldValue {
                onStateChange?(state)
            }
        }
    }
    var onStateChange: ((State) -> Void)?

    var isActive: Bool { state != .idle }

    var elapsed: TimeInterval? {
        startedAt.map { Date().timeIntervalSince($0) }
    }

    private let settings: Settings
    private let local: LocalEngine
    private var mic: MeetingMicCapture?
    private var systemTap: SystemAudioTap?
    private var pipeline: MeetingPipeline?
    private var file: MeetingFile?
    private var transcription: Task<Void, Never>?
    private var startedAt: Date?
    private var dropped = 0

    init(settings: Settings, local: LocalEngine) {
        self.settings = settings
        self.local = local
    }

    func start() async throws {
        guard state == .idle else {
            throw MeetingError("a meeting is already being transcribed")
        }
        guard settings.engine == .local else {
            throw MeetingError("meetings are transcribed on this Mac; switch Transcribe on to This Mac first")
        }
        guard await local.status == .ready else {
            throw MeetingError("the local model is not ready yet")
        }
        let (stream, continuation) = AsyncStream<MeetingPipeline.Job>.makeStream()
        let pipeline = MeetingPipeline(continuation: continuation)
        let mic = MeetingMicCapture()
        let tap = SystemAudioTap()
        do {
            // The microphone first, so this app is an audio process the tap can exclude.
            try mic.start { samples, rate in pipeline.feed(samples, rate: rate, speaker: .me) }
            try tap.start { samples, rate in pipeline.feed(samples, rate: rate, speaker: .others) }
            let started = Date()
            let file = try MeetingFile.create(startedAt: started, microphone: mic.deviceName)
            self.mic = mic
            systemTap = tap
            self.pipeline = pipeline
            self.file = file
            startedAt = started
            dropped = 0
            transcription = Task { [weak self] in await self?.transcribe(stream) }
            state = .recording
            Log.info(
                "meeting transcription started: \(file.url.lastPathComponent), microphone \(mic.deviceName) "
                    + "at \(Int(mic.rate)) Hz, system audio at \(Int(tap.rate)) Hz")
        } catch {
            mic.stop()
            tap.stop()
            _ = pipeline.finish()
            throw error
        }
    }

    /// Stops both captures, transcribes whatever is still queued, and closes the file.
    @discardableResult
    func stop() async -> URL? {
        guard state == .recording, let startedAt, let file else {
            return nil
        }
        state = .finishing
        mic?.stop()
        mic = nil
        systemTap?.stop()
        systemTap = nil
        let peaks = pipeline?.finish() ?? [:]
        pipeline = nil
        await transcription?.value
        transcription = nil
        let ended = Date()
        file.finish(
            MeetingTranscript.footer(
                endedAt: ended, duration: ended.timeIntervalSince(startedAt), droppedSegments: dropped))
        self.file = nil
        self.startedAt = nil
        state = .idle
        var heard = "meeting transcription stopped: \(file.url.lastPathComponent); "
            + "peak levels Me \(peaks[.me] ?? 0), Others \(peaks[.others] ?? 0)"
        if (peaks[.others] ?? 0) == 0 {
            heard += "; the Others track was silent, which a refused system audio permission also looks like"
        }
        Log.info(heard)
        return file.url
    }

    /// The app is going away: keep everything already in the file, without waiting for the
    /// segments still queued. Their absence is not counted; the log says the quit cut it short.
    func stopForTermination() {
        guard state != .idle, let startedAt, let file else {
            return
        }
        mic?.stop()
        systemTap?.stop()
        transcription?.cancel()
        _ = pipeline?.finish()
        let ended = Date()
        file.finish(
            MeetingTranscript.footer(
                endedAt: ended, duration: ended.timeIntervalSince(startedAt), droppedSegments: dropped))
        Log.warning("meeting transcription ended by quit; the segments still queued are missing")
    }

    private func transcribe(_ stream: AsyncStream<MeetingPipeline.Job>) async {
        for await job in stream {
            do {
                let text = try await local.transcribe(job.samples)
                guard !text.isEmpty else {
                    continue
                }
                file?.append(line: MeetingTranscript.line(at: job.start, speaker: job.speaker, text: text))
                Log.info(
                    "meeting segment [\(MeetingTranscript.timestamp(job.start))] \(job.speaker.label): "
                        + "\(text.count) chars")
            } catch {
                dropped += 1
                Log.error("meeting segment at \(MeetingTranscript.timestamp(job.start)) failed: \(error)")
            }
        }
    }
}
