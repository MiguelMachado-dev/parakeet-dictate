import Accelerate
import Foundation

// A port of src/parakeet_dictate/meter.py: live microphone level and spectrum for the overlay,
// fed from the audio callback.

/// Single writer (the audio callback) and single reader (the overlay's display link).
public final class LevelMeter: @unchecked Sendable {
    public static let hopSeconds = 0.02
    public static let levelHistory = 96
    public static let spectrumBands = 24
    static let fftSize = 2048
    static let bandRange = (low: 80.0, high: 8000.0)
    static let floorDB: Float = -60
    /// The ceiling follows the loudest recent hop, but never drops below this, so a quiet mic still
    /// fills the scale and room noise does not.
    static let minCeilingDB: Float = -35
    static let peakDecayDBPerSecond: Float = 3
    /// Speech sits in the top quarter of the dB range (fixture median -21 dBFS, peak -13), so a
    /// linear map pins the bars near full. The curve spreads that quarter over most of the scale.
    static let curve: Float = 2.5
    static let attack: Float = 0.6
    static let release: Float = 0.15
    static let eps = 1e-12

    public let rate: Double
    private let hop: Int
    private let window: [Float]
    private let windowPower: Double
    /// The band of each rfft bin, or -1 outside 80 Hz to 8 kHz.
    private let bandOfBin: [Int]
    private let fft: FFTSetup
    private let lock = NSLock()

    private var levels = [Float](repeating: 0, count: levelHistory)
    private var spectrum = [Float](repeating: 0, count: spectrumBands)
    /// The last fftSize samples, oldest first.
    private var tail = [Float](repeating: 0, count: fftSize)
    private var pending = 0
    private var level: Float = 0
    private var peakDB = minCeilingDB

    public init(rate: Double) {
        self.rate = rate
        hop = max(1, Int((rate * Self.hopSeconds).rounded(.toNearestOrEven)))
        let n = Self.fftSize
        // np.hanning: the symmetric Hann window.
        window = (0..<n).map { Float(0.5 - 0.5 * cos(2 * Double.pi * Double($0) / Double(n - 1))) }
        windowPower = window.reduce(0) { $0 + Double($1) * Double($1) }
        let edges = (0...Self.spectrumBands).map {
            Self.bandRange.low * pow(Self.bandRange.high / Self.bandRange.low, Double($0) / Double(Self.spectrumBands))
        }
        bandOfBin = (0...n / 2).map { bin in
            let frequency = Double(bin) * rate / Double(n)
            // np.digitize(freqs, edges) - 1
            return edges.lastIndex(where: { $0 <= frequency }) ?? -1
        }
        fft = vDSP_create_fftsetup(vDSP_Length(log2(Double(n))), FFTRadix(kFFTRadix2))!
    }

    deinit {
        vDSP_destroy_fftsetup(fft)
    }

    public func reset() {
        lock.withLock {
            levels = [Float](repeating: 0, count: Self.levelHistory)
            spectrum = [Float](repeating: 0, count: Self.spectrumBands)
            tail = [Float](repeating: 0, count: Self.fftSize)
            pending = 0
            level = 0
            peakDB = Self.minCeilingDB
        }
    }

    public func feed(_ block: UnsafeBufferPointer<Float>) {
        lock.withLock {
            var start = 0
            while start < block.count {
                let take = min(block.count - start, hop - pending)
                tail.removeFirst(take)
                tail.append(contentsOf: block[start..<start + take])
                pending += take
                start += take
                if pending == hop {
                    pending = 0
                    analyzeHop()
                }
            }
        }
    }

    public func feed(_ block: [Float]) {
        block.withUnsafeBufferPointer { feed($0) }
    }

    public func snapshot() -> (levels: [Float], spectrum: [Float]) {
        lock.withLock { (levels, spectrum) }
    }

    private static func db(_ power: Double) -> Float {
        Float(10 * log10(power + eps))
    }

    private func analyzeHop() {
        let n = Self.fftSize
        var sum = 0.0
        for sample in tail[(n - hop)...] {
            sum += Double(sample) * Double(sample)
        }
        let rmsDB = Self.db(sum / Double(hop))
        peakDB = max(peakDB - Self.peakDecayDBPerSecond * Float(Self.hopSeconds), rmsDB)
        let ceiling = max(Self.minCeilingDB, peakDB)

        let target = pow(min(max((rmsDB - Self.floorDB) / (ceiling - Self.floorDB), 0), 1), Self.curve)
        level += (target - level) * (target > level ? Self.attack : Self.release)
        levels.removeFirst()
        levels.append(level)

        // Scaled so the band powers sum to the frame's mean power, which keeps bands on the
        // same dB scale as the level.
        let power = powerSpectrum()
        let scale = 2.0 / (Double(n) * windowPower)
        var bands = [Double](repeating: 0, count: Self.spectrumBands)
        for (bin, band) in bandOfBin.enumerated() where band >= 0 && band < Self.spectrumBands {
            bands[band] += power[bin] * scale
        }
        for band in 0..<Self.spectrumBands {
            let bandTarget = pow(min(max((Self.db(bands[band]) - Self.floorDB) / (ceiling - Self.floorDB), 0), 1), Self.curve)
            let rate = bandTarget > spectrum[band] ? Self.attack : Self.release
            spectrum[band] += (bandTarget - spectrum[band]) * rate
        }
    }

    /// |rfft(tail * window)|² for bins 0...n/2.
    private func powerSpectrum() -> [Double] {
        let n = Self.fftSize
        var windowed = [Float](repeating: 0, count: n)
        vDSP_vmul(tail, 1, window, 1, &windowed, 1, vDSP_Length(n))
        var real = [Float](repeating: 0, count: n / 2)
        var imag = [Float](repeating: 0, count: n / 2)
        var power = [Double](repeating: 0, count: n / 2 + 1)
        real.withUnsafeMutableBufferPointer { realPointer in
            imag.withUnsafeMutableBufferPointer { imagPointer in
                var split = DSPSplitComplex(realp: realPointer.baseAddress!, imagp: imagPointer.baseAddress!)
                windowed.withUnsafeBytes {
                    vDSP_ctoz($0.bindMemory(to: DSPComplex.self).baseAddress!, 2, &split, 1, vDSP_Length(n / 2))
                }
                vDSP_fft_zrip(fft, &split, 1, vDSP_Length(log2(Double(n))), FFTDirection(kFFTDirection_Forward))
                // zrip returns twice the DFT, with the Nyquist bin packed into imagp[0].
                power[0] = pow(Double(split.realp[0]) / 2, 2)
                power[n / 2] = pow(Double(split.imagp[0]) / 2, 2)
                for k in 1..<n / 2 {
                    power[k] = (pow(Double(split.realp[k]), 2) + pow(Double(split.imagp[k]), 2)) / 4
                }
            }
        }
        return power
    }
}
