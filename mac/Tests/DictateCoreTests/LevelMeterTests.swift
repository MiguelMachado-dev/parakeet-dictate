import Foundation
import Testing

@testable import DictateCore

// A port of tests/test_meter.py, plus a spectrum checked against the Python meter.

private let rate = 44_100.0

private func tone(_ db: Double, _ seconds: Double, hz: Double = 440, rate: Double = rate) -> [Float] {
    let amplitude = 2.0.squareRoot() * pow(10, db / 20)
    return (0..<Int((rate * seconds).rounded())).map { Float(amplitude * sin(2 * .pi * hz * Double($0) / rate)) }
}

@Test func silenceReadsZero() {
    let meter = LevelMeter(rate: rate)
    meter.feed([Float](repeating: 0, count: Int(rate)))
    let (levels, spectrum) = meter.snapshot()
    #expect(levels.count == LevelMeter.levelHistory && spectrum.count == LevelMeter.spectrumBands)
    #expect(levels.max() == 0 && spectrum.max() == 0)
}

@Test func roomNoiseBelowTheFloorReadsZero() {
    let meter = LevelMeter(rate: rate)
    meter.feed(tone(-87, 1))
    #expect(meter.snapshot().levels.max() == 0)
}

@Test func aQuietMicStillFillsMostOfTheScale() {
    let meter = LevelMeter(rate: rate)
    meter.feed(tone(-40, 1))
    #expect(abs(meter.snapshot().levels.last! - pow(20 / 25, LevelMeter.curve)) < 0.02)
}

@Test func aLoudMicMovesTheCeilingUpToItsPeak() {
    let meter = LevelMeter(rate: rate)
    meter.feed(tone(-12, 1))
    #expect(abs(meter.snapshot().levels.last! - 1) < 0.02)
}

@Test func historyIsOldestFirst() {
    let meter = LevelMeter(rate: rate)
    meter.feed([Float](repeating: 0, count: Int(rate)))
    meter.feed(tone(-20, 10 * LevelMeter.hopSeconds))
    let levels = meter.snapshot().levels
    #expect(levels.first == 0 && levels.last! > 0.5)
}

@Test func theToneLandsInItsBand() {
    let meter = LevelMeter(rate: rate)
    meter.feed(tone(-20, 0.5, hz: 1000))
    let spectrum = meter.snapshot().spectrum
    let loudest = spectrum.indices.max { spectrum[$0] < spectrum[$1] }!
    // np.digitize(1000, np.geomspace(80, 8000, 25)) - 1
    #expect(loudest == Int((log(1000.0 / 80) / log(100) * 24).rounded(.down)))
}

@Test func feedingInOddBlockSizesMatchesOneBlock() {
    let signal = tone(-25, 0.7)
    let whole = LevelMeter(rate: rate)
    let pieces = LevelMeter(rate: rate)
    whole.feed(signal)
    let size = signal.count / 37 + 1
    for start in stride(from: 0, to: signal.count, by: size) {
        pieces.feed(Array(signal[start..<min(start + size, signal.count)]))
    }
    #expect(zip(whole.snapshot().levels, pieces.snapshot().levels).allSatisfy { abs($0 - $1) < 1e-6 })
}

@Test func resetClearsHistory() {
    let meter = LevelMeter(rate: rate)
    meter.feed(tone(-20, 0.5))
    meter.reset()
    #expect(meter.snapshot().levels.max() == 0)
}

@Test func spectrumMatchesThePythonMeter() {
    let a = tone(-24, 0.6, hz: 300, rate: 48_000)
    let b = tone(-33, 0.6, hz: 2500, rate: 48_000)
    let meter = LevelMeter(rate: 48_000)
    meter.feed(zip(a, b).map { $0 + $1 })
    // LevelMeter(48000).feed(...).snapshot()[1] from src/parakeet_dictate/meter.py, rounded to 5 places.
    let python: [Float] = [
        0.00018, 0.0002, 0.00025, 0.0003, 0.00039, 0.00102, 0.93969, 0.39773, 0.00071, 0.00014, 0.00002, 0, 0, 0, 0, 0,
        0, 0.46095, 0.08769, 0, 0, 0, 0, 0,
    ]
    let spectrum = meter.snapshot().spectrum
    #expect(zip(spectrum, python).allSatisfy { abs($0 - $1) < 0.002 }, "\(spectrum)")
}

@Test func aBlockPlaysOutAtItsRealPace() {
    let paced = LevelMeter(rate: rate)
    let newest = LevelMeter(rate: rate)
    paced.feed([Float](repeating: 0, count: Int(rate)), at: -1)
    newest.feed([Float](repeating: 0, count: Int(rate)))
    // Five hops arrive at once at t = 0.
    let block = tone(-20, 5 * LevelMeter.hopSeconds)
    paced.feed(block, at: 0)
    newest.feed(block)
    #expect(paced.snapshot(at: 0).levels.last == 0)
    #expect(paced.snapshot(at: 0.11) == newest.snapshot())
    #expect(paced.snapshot(at: 10) == newest.snapshot())
    // Halfway through a hop, the levels blend the hops on either side.
    let one = paced.snapshot(at: LevelMeter.hopSeconds).levels.last!
    let two = paced.snapshot(at: 2 * LevelMeter.hopSeconds).levels.last!
    let between = paced.snapshot(at: 1.5 * LevelMeter.hopSeconds).levels.last!
    #expect(one > 0 && two > one && abs(between - (one + two) / 2) < 1e-6)
}
