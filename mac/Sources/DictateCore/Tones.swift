import Foundation

/// The four short cues of the Windows app (`Cues` in app.py), as WAV files.
public enum Tones {
    public static let rate = 44_100
    static let noteSeconds = 0.06
    static let gapSeconds = 0.02
    static let fadeSeconds = 0.008
    static let volume: Float = 0.12

    public static func notes(_ cue: Cue) -> [Double] {
        switch cue {
        case .start: return [600, 900]
        case .stop: return [900, 600]
        case .cancel: return [450]
        case .error: return [300, 300]
        }
    }

    public static func wav(_ cue: Cue) -> Data {
        Wav.encode(samples(cue), rate: rate)
    }

    static func samples(_ cue: Cue) -> [Float] {
        let n = Int(noteSeconds * Double(rate))
        let fade = Double(Int(fadeSeconds * Double(rate)))
        var samples: [Float] = []
        for hz in notes(cue) {
            for i in 0..<n {
                let envelope = min(1, Double(i) / fade, Double(n - i) / fade)
                samples.append(volume * Float(envelope * sin(2 * .pi * hz * Double(i) / Double(rate))))
            }
            samples.append(contentsOf: repeatElement(0, count: Int(gapSeconds * Double(rate))))
        }
        return samples
    }
}
