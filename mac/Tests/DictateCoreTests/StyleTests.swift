import Foundation
import Testing

@testable import DictateCore

// A port of tests/test_styles.py, plus a render-time budget for the overlay's 60 fps.

/// A fixed stand-in for the Python test's default_rng(7) draws, which Swift cannot reproduce.
private func uniform(_ count: Int, _ low: Float, _ high: Float, seed: UInt64) -> [Float] {
    var state = seed
    return (0..<count).map { _ in
        state = state &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
        return low + (high - low) * Float(state >> 40) / Float(1 << 24)
    }
}

private let loud = (uniform(LevelMeter.levelHistory, 0.3, 1.0, seed: 7), uniform(LevelMeter.spectrumBands, 0.2, 1.0, seed: 8))
private let silent = (
    [Float](repeating: 0, count: LevelMeter.levelHistory), [Float](repeating: 0, count: LevelMeter.spectrumBands)
)

private func frame(
    _ phase: Phase, _ presence: Double, _ scale: Double, handsFree: Bool = false, audio: ([Float], [Float]) = loud
) -> OverlayFrame {
    let elapsed = [.done, .cancelled, .failed].contains(phase) ? OverlayCanvas.beat / 2 : 0.7
    return OverlayFrame(
        phase: phase, phaseElapsed: elapsed, presence: presence, handsFree: handsFree, recordingElapsed: 2.0,
        levels: audio.0, spectrum: audio.1, scale: scale)
}

private func alpha(_ image: RGBAImage) -> [UInt8] {
    stride(from: 3, to: image.pixels.count, by: 4).map { image.pixels[$0] }
}

@Test(arguments: OverlayStyle.allCases, Phase.allCases)
func everyPhaseDrawsInsideTheCanvas(style: OverlayStyle, phase: Phase) {
    for scale in [1.0, 1.5] {
        let f = frame(phase, 1.0, scale, handsFree: phase == .recording)
        let image = style.render(f)
        #expect(image.width == f.width && image.height == f.height && image.pixels.count == f.width * f.height * 4)
        let a = alpha(image)
        #expect(a.max()! > 200, "the body is visible")
        let w = image.width
        let h = image.height
        let border = (0..<w).flatMap { [a[$0], a[(h - 1) * w + $0]] } + (0..<h).flatMap { [a[$0 * w], a[$0 * w + w - 1]] }
        #expect(border.max() == 0, "nothing reaches the canvas edge")
    }
}

@Test(arguments: OverlayStyle.allCases)
func presenceZeroIsFullyTransparent(style: OverlayStyle) {
    #expect(style.render(frame(.recording, 0.0, 1.0)).pixels.allSatisfy { $0 == 0 })
}

@Test(arguments: OverlayStyle.allCases)
func recordingReactsToTheVoice(style: OverlayStyle) {
    let loudImage = style.render(frame(.recording, 1.0, 1.0))
    let silentImage = style.render(frame(.recording, 1.0, 1.0, audio: silent))
    let total = zip(loudImage.pixels, silentImage.pixels).reduce(0) { $0 + abs(Int($1.0) - Int($1.1)) }
    #expect(Double(total) / Double(loudImage.pixels.count) > 1.0)
}

/// A hands-free recording at full presence on a Retina screen: the largest frame either style draws.
/// The overlay draws at 60 fps, so a frame must take well under its 16 ms.
@Test(arguments: OverlayStyle.allCases)
func renderingFitsTheFrameBudget(style: OverlayStyle) {
    let f = OverlayFrame(
        phase: .recording, phaseElapsed: 0.7, presence: 1, handsFree: true, recordingElapsed: 2.7, levels: loud.0,
        spectrum: loud.1, scale: 2)
    let clock = ContinuousClock()
    var times: [Duration] = []
    for _ in 0..<50 {
        let start = clock.now
        _ = style.render(f)
        times.append(clock.now - start)
    }
    let median = times.sorted()[times.count / 2]
    #if DEBUG
        _ = median
    #else
        #expect(median < .milliseconds(5), "\(median)")
    #endif
}
