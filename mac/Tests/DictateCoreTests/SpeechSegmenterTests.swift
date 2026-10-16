import Foundation
import Testing

@testable import DictateCore

private let rate = 16_000.0

private func tone(_ db: Double, _ seconds: Double) -> [Float] {
    let amplitude = 2.0.squareRoot() * pow(10, db / 20)
    return (0..<Int((rate * seconds).rounded())).map { Float(amplitude * sin(2 * .pi * 440 * Double($0) / rate)) }
}

private func silence(_ seconds: Double) -> [Float] {
    [Float](repeating: 0, count: Int((rate * seconds).rounded()))
}

private func seconds(_ segment: SpeechSegmenter.Segment) -> Double {
    Double(segment.samples.count) / rate
}

@Test func silenceProducesNoSegments() {
    let segmenter = SpeechSegmenter(rate: rate)
    #expect(segmenter.feed(silence(5)).isEmpty)
    #expect(segmenter.flush().isEmpty)
}

@Test func quietNoiseIsNotSpeech() {
    let segmenter = SpeechSegmenter(rate: rate)
    #expect(segmenter.feed(tone(-60, 3) + silence(2)).isEmpty)
    #expect(segmenter.flush().isEmpty)
}

@Test func speechThenSilenceProducesOneSegmentWithPreRoll() {
    let segmenter = SpeechSegmenter(rate: rate)
    let segments = segmenter.feed(silence(1) + tone(-20, 2) + silence(2))
    #expect(segments.count == 1)
    // 0.3 s of pre-roll before the tone at 1 s, and a 0.2 s tail after it.
    #expect(abs(segments[0].start - 0.7) < 0.15, "\(segments[0].start)")
    #expect(abs(seconds(segments[0]) - 2.5) < 0.15, "\(seconds(segments[0]))")
}

@Test func twoUtterancesSplitByALongPause() {
    let segmenter = SpeechSegmenter(rate: rate)
    let segments = segmenter.feed(tone(-20, 1) + silence(2) + tone(-20, 1) + silence(2))
    #expect(segments.count == 2)
    #expect(abs(segments[0].start) < 0.15, "\(segments[0].start)")
    #expect(abs(seconds(segments[0]) - 1.2) < 0.15, "\(seconds(segments[0]))")
    #expect(abs(segments[1].start - 2.7) < 0.15, "\(segments[1].start)")
}

@Test func aShortBlipIsDropped() {
    let segmenter = SpeechSegmenter(rate: rate)
    #expect(segmenter.feed(silence(1) + tone(-20, 0.2) + silence(2)).isEmpty)
}

@Test func longSpeechIsCutAndContinues() {
    let segmenter = SpeechSegmenter(rate: rate, maxSegmentSeconds: 2)
    let segments = segmenter.feed(tone(-20, 5) + silence(2))
    #expect(segments.count == 2)
    // No quiet frame to cut on, so the first segment runs to the hard limit of 1.5x the maximum.
    #expect(abs(seconds(segments[0]) - 3.0) < 0.15, "\(seconds(segments[0]))")
    #expect(abs(segments[1].start - 3.0) < 0.15, "\(segments[1].start)")
}

@Test func flushEmitsTheSegmentStillOpen() {
    let segmenter = SpeechSegmenter(rate: rate)
    #expect(segmenter.feed(tone(-20, 1)).isEmpty)
    let segments = segmenter.flush()
    #expect(segments.count == 1)
    #expect(abs(seconds(segments[0]) - 1.0) < 0.15, "\(seconds(segments[0]))")
}

@Test func feedingInOddPiecesMatchesOneBlock() {
    let signal = silence(0.5) + tone(-20, 1.5) + silence(1.5) + tone(-20, 1) + silence(1.5)
    let whole = SpeechSegmenter(rate: rate)
    let pieces = SpeechSegmenter(rate: rate)
    var fromWhole = whole.feed(signal)
    fromWhole += whole.flush()
    var fromPieces: [SpeechSegmenter.Segment] = []
    for start in stride(from: 0, to: signal.count, by: 999) {
        fromPieces += pieces.feed(Array(signal[start..<min(start + 999, signal.count)]))
    }
    fromPieces += pieces.flush()
    #expect(fromWhole.count == 2)
    #expect(fromWhole == fromPieces)
}
