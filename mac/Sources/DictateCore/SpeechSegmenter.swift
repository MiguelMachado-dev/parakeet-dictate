import Foundation

/// Cuts a continuous mono stream into utterance-sized segments for meeting transcription, so a
/// meeting is transcribed as it goes instead of from one growing buffer at the end. A segment
/// starts when a 100 ms frame's RMS reaches `speechRMS`, keeps a short pre-roll of the frames
/// before it so the first word is not clipped, and ends after `silenceSeconds` of quiet (keeping
/// a 200 ms tail) or near `maxSegmentSeconds`, where it waits for a quiet frame and gives up
/// waiting at one and a half times that. Speech shorter than `minSpeechSeconds` is dropped, so
/// clicks and coughs never reach the model. One segmenter per track, fed from one thread; the
/// lock only keeps a stray second feeder from corrupting the state.
public final class SpeechSegmenter: @unchecked Sendable {
    public struct Segment: Equatable {
        /// Seconds from the first sample fed to this segment's first sample.
        public let start: Double
        public let samples: [Float]
    }

    public static let frameSeconds = 0.1

    public let rate: Double
    private let speechRMS: Float
    private let silenceFramesNeeded: Int
    private let minSpeechSeconds: Double
    private let maxSegmentSeconds: Double
    private let preRollFrames: Int
    private let tailFrames = 2

    private let lock = NSLock()
    private let frameSize: Int
    private var pending: [Float] = []
    private var framesProcessed = 0
    /// The last pre-roll frames, oldest first, whatever state they were heard in.
    private var history: [[Float]] = []
    private var inSegment = false
    private var segmentSamples: [Float] = []
    private var segmentStartSample = 0
    private var speechFrames = 0
    private var silenceRun = 0
    /// Where in `segmentSamples` the last speech frame ended, for trimming the hangover silence.
    private var lastSpeechEnd = 0

    public init(
        rate: Double, speechRMS: Float = 0.01, silenceSeconds: Double = 0.8, minSpeechSeconds: Double = 0.3,
        maxSegmentSeconds: Double = 30, preRollSeconds: Double = 0.3
    ) {
        self.rate = rate
        self.speechRMS = speechRMS
        silenceFramesNeeded = max(1, Int((silenceSeconds / Self.frameSeconds).rounded()))
        self.minSpeechSeconds = minSpeechSeconds
        self.maxSegmentSeconds = maxSegmentSeconds
        preRollFrames = max(0, Int((preRollSeconds / Self.frameSeconds).rounded()))
        frameSize = max(1, Int((rate * Self.frameSeconds).rounded()))
    }

    /// The segments this block completes, in order.
    public func feed(_ samples: [Float]) -> [Segment] {
        lock.withLock {
            var emitted: [Segment] = []
            pending.append(contentsOf: samples)
            while pending.count >= frameSize {
                let frame = Array(pending.prefix(frameSize))
                pending.removeFirst(frameSize)
                process(frame, into: &emitted)
            }
            return emitted
        }
    }

    /// The segment still open when the stream ends, if it holds enough speech.
    public func flush() -> [Segment] {
        lock.withLock {
            var emitted: [Segment] = []
            if inSegment {
                segmentSamples.append(contentsOf: pending)
                endSegment(trimTo: segmentSamples.count, into: &emitted)
            }
            pending = []
            return emitted
        }
    }

    private func process(_ frame: [Float], into emitted: inout [Segment]) {
        let frameStart = framesProcessed * frameSize
        framesProcessed += 1
        var sum = 0.0
        for sample in frame {
            sum += Double(sample) * Double(sample)
        }
        let isSpeech = Float((sum / Double(frame.count)).squareRoot()) >= speechRMS

        if !inSegment {
            if isSpeech {
                inSegment = true
                segmentSamples = history.flatMap { $0 } + frame
                segmentStartSample = frameStart - history.count * frameSize
                speechFrames = 1
                silenceRun = 0
                lastSpeechEnd = segmentSamples.count
            }
        } else {
            segmentSamples.append(contentsOf: frame)
            if isSpeech {
                speechFrames += 1
                silenceRun = 0
                lastSpeechEnd = segmentSamples.count
            } else {
                silenceRun += 1
            }
            let seconds = Double(segmentSamples.count) / rate
            if silenceRun >= silenceFramesNeeded {
                endSegment(trimTo: lastSpeechEnd + tailFrames * frameSize, into: &emitted)
            } else if seconds >= maxSegmentSeconds * 1.5 || (seconds >= maxSegmentSeconds && !isSpeech) {
                endSegment(trimTo: segmentSamples.count, into: &emitted)
                // The speaker has not paused: the next segment starts with the next frame.
                inSegment = true
                segmentStartSample = frameStart + frameSize
            }
        }

        history.append(frame)
        if history.count > preRollFrames {
            history.removeFirst(history.count - preRollFrames)
        }
    }

    private func endSegment(trimTo: Int, into emitted: inout [Segment]) {
        let keep = min(trimTo, segmentSamples.count)
        if Double(speechFrames) * Self.frameSeconds >= minSpeechSeconds, keep > 0 {
            emitted.append(
                Segment(start: Double(segmentStartSample) / rate, samples: Array(segmentSamples.prefix(keep))))
        }
        inSegment = false
        segmentSamples = []
        speechFrames = 0
        silenceRun = 0
        lastSpeechEnd = 0
    }
}
