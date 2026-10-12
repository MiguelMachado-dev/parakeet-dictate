// A port of src/parakeet_dictate/overlay_frame.py: what the overlay draws on each frame.
// A style is a pure function of an OverlayFrame.

public enum OverlayCanvas {
    /// Points at scale 1. The style draws everything inside it, glow and shadow included, so the window never resizes.
    public static let width = 400.0
    public static let height = 120.0
    public static let fps = 60.0
    /// How long the done, cancelled, and failed beats play before the window fades out.
    public static let beat = 0.45
}

public enum Phase: String, CaseIterable, Sendable {
    case recording
    case transcribing
    case polishing
    case done
    case cancelled
    case failed
}

public struct OverlayFrame: Sendable {
    public var phase: Phase
    public var phaseElapsed: Double
    /// 0 is fully gone, 1 is fully shown. The window eases it on show and hide; the style decides
    /// what partial presence looks like.
    public var presence: Double
    public var handsFree: Bool
    /// Seconds since the recording began, frozen once it stops.
    public var recordingElapsed: Double
    /// LevelMeter.levelHistory values in 0...1, oldest first, one per meter hop.
    public var levels: [Float]
    /// LevelMeter.spectrumBands values in 0...1, low to high frequency.
    public var spectrum: [Float]
    /// Pixels per point: the screen's backing scale factor.
    public var scale: Double

    public init(
        phase: Phase, phaseElapsed: Double, presence: Double, handsFree: Bool, recordingElapsed: Double,
        levels: [Float], spectrum: [Float], scale: Double = 1
    ) {
        self.phase = phase
        self.phaseElapsed = phaseElapsed
        self.presence = presence
        self.handsFree = handsFree
        self.recordingElapsed = recordingElapsed
        self.levels = levels
        self.spectrum = spectrum
        self.scale = scale
    }

    /// The canvas in pixels, rounded like Python's round().
    public var width: Int { Int((OverlayCanvas.width * scale).rounded(.toNearestOrEven)) }
    public var height: Int { Int((OverlayCanvas.height * scale).rounded(.toNearestOrEven)) }
}

/// Premultiplied RGBA, 8 bits per channel, rows from the top. What a CGImage with premultipliedLast alpha takes.
public struct RGBAImage: Sendable {
    public let width: Int
    public let height: Int
    public var pixels: [UInt8]

    public init(width: Int, height: Int) {
        self.width = width
        self.height = height
        self.pixels = [UInt8](repeating: 0, count: width * height * 4)
    }
}

public enum OverlayStyle: String, CaseIterable, Sendable {
    /// Voice ribbons in budgerigar colours, in a night-blue capsule. The default.
    case fluid
    /// Mirrored bars in a smoked-glass capsule.
    case bars

    public func render(_ frame: OverlayFrame) -> RGBAImage {
        switch self {
        case .fluid: return Fluid.render(frame)
        case .bars: return Bars.render(frame)
        }
    }
}
