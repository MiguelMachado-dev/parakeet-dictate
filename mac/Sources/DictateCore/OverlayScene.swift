// A port of src/parakeet_dictate/overlay_scene.py: what the overlay shows and when. Maps dictation
// transitions to commands and plays them out over time. Pure and clock-free, so tests drive it without a window.

public enum OverlayTiming {
    /// A recording only appears after this long, so the hotkey pressed as part of a shortcut such as
    /// Command-C never flashes the window. The start tone is the instant feedback.
    public static let showDelay = 0.25
    public static let fadeIn = 0.15
    public static let fadeOut = 0.2
    /// Points between the bottom of the canvas and the bottom of the visible frame.
    public static let bottomMargin = 48.0
    /// Points between the top of the canvas and the bottom of the menu bar. Smaller than the bottom margin,
    /// so the capsule sits close under the menu bar rather than floating in the window below it.
    public static let topMargin = 16.0
}

/// Where on the screen the overlay shows. The Windows app has only the bottom.
public enum OverlayPosition: String, CaseIterable, Sendable {
    case bottom
    case top
}

public enum OverlayCommand: Equatable, Sendable {
    case show(Phase, handsFree: Bool = false)
    /// Ends the dictation: plays the beat for OverlayCanvas.beat, then fades out. No beat fades out at once.
    case finish(beat: Phase?)
    /// Ends the dictation with no beat and no fade.
    case vanish
}

public func overlayCommand(previous: State, state: State, event: Event, effects: [Effect]) -> OverlayCommand? {
    switch state {
    case .recording(let mode, _):
        return .show(.recording, handsFree: mode == .toggle)
    case .transcribing, .awaitingRelease:
        return .show(.transcribing)
    case .polishing:
        return .show(.polishing)
    case .idle:
        if case .idle = previous {
            return nil
        }
        if event == .otherKeyDown {
            return .vanish
        }
        return .finish(beat: beat(effects))
    }
}

private func beat(_ effects: [Effect]) -> Phase? {
    for effect in effects {
        switch effect {
        case .paste: return .done
        case .playSound(.cancel): return .cancelled
        case .playSound(.error): return .failed
        default: continue
        }
    }
    return nil
}

/// Where the canvas sits, in points with the origin at the bottom left (AppKit), and its pixels per point.
public struct Placement: Equatable, Sendable {
    public var x: Double
    public var y: Double
    public var width: Double
    public var height: Double
    public var scale: Double

    public init(x: Double, y: Double, width: Double, height: Double, scale: Double) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
        self.scale = scale
    }

    /// Bottom center of a screen's visible frame, which leaves out the Dock and the menu bar.
    public static func bottomCenter(
        visibleX: Double, visibleY: Double, visibleWidth: Double, scale: Double
    ) -> Placement {
        let width = OverlayCanvas.width
        let height = OverlayCanvas.height
        return Placement(
            x: (visibleX + (visibleWidth - width) / 2).rounded(.down), y: visibleY + OverlayTiming.bottomMargin,
            width: width, height: height, scale: scale)
    }

    /// Top center of a screen's visible frame, just under the menu bar. `visibleTop` is the frame's maxY.
    public static func topCenter(
        visibleX: Double, visibleTop: Double, visibleWidth: Double, scale: Double
    ) -> Placement {
        var placement = bottomCenter(visibleX: visibleX, visibleY: 0, visibleWidth: visibleWidth, scale: scale)
        placement.y = (visibleTop - OverlayTiming.topMargin - placement.height).rounded(.down)
        return placement
    }
}

/// One dictation on screen, from its fade-in to the end of its fade-out.
private final class Appearance {
    let showsAt: Double
    let recordingStarted: Double
    let placement: Placement
    var phase: Phase
    var phaseStarted: Double
    var handsFree: Bool
    var recordingEnded: Double?
    var fadesAt: Double?
    /// Presence at showsAt: above 0 when the previous dictation was still fading out.
    var riseFrom = 0.0

    init(showsAt: Double, recordingStarted: Double, placement: Placement, phase: Phase, phaseStarted: Double, handsFree: Bool) {
        self.showsAt = showsAt
        self.recordingStarted = recordingStarted
        self.placement = placement
        self.phase = phase
        self.phaseStarted = phaseStarted
        self.handsFree = handsFree
    }

    func presence(_ now: Double) -> Double {
        func rising(_ t: Double) -> Double {
            min(1.0, riseFrom + (t - showsAt) / OverlayTiming.fadeIn)
        }
        if now < showsAt {
            return 0
        }
        guard let fadesAt, now >= fadesAt else {
            return rising(now)
        }
        return max(0, rising(fadesAt) - (now - fadesAt) / OverlayTiming.fadeOut)
    }

    func gone(_ now: Double) -> Bool {
        guard let fadesAt else {
            return false
        }
        return now >= fadesAt && presence(now) <= 0
    }
}

public final class OverlayScene {
    /// The latest dictation, possibly still waiting for the show delay.
    private var current: Appearance?
    /// The dictation before it, still ending on screen while `current` waits.
    private var outgoing: Appearance?

    public init() {}

    /// `placement` is where a dictation that starts with this command will show.
    public func apply(_ command: OverlayCommand, at t: Double, placement: Placement) {
        advance(t)
        guard let current, current.fadesAt == nil else {
            if case .show(let phase, let handsFree) = command {
                outgoing = current
                let showsAt = t + OverlayTiming.showDelay
                self.current = Appearance(
                    showsAt: showsAt, recordingStarted: t, placement: placement, phase: phase, phaseStarted: showsAt,
                    handsFree: handsFree)
            }
            return
        }
        switch command {
        case .show(let phase, let handsFree):
            if phase != current.phase {
                current.phase = phase
                current.phaseStarted = max(t, current.showsAt)
            }
            if phase != .recording && current.recordingEnded == nil {
                current.recordingEnded = t
            }
            current.handsFree = current.handsFree || handsFree
        case .finish, .vanish:
            if t < current.showsAt {
                // Never shown, so it never will be: whatever was ending on screen plays on untouched.
                self.current = outgoing
                outgoing = nil
            } else if case .finish(let beat) = command {
                if current.recordingEnded == nil {
                    current.recordingEnded = t
                }
                if let beat {
                    current.phase = beat
                    current.phaseStarted = t
                    current.fadesAt = t + OverlayCanvas.beat
                } else {
                    current.fadesAt = t
                }
            } else {
                self.current = nil
            }
        }
    }

    /// The frame to draw at `now`, or nil when the window should be hidden.
    public func frame(at now: Double, levels: [Float], spectrum: [Float]) -> (OverlayFrame, Placement)? {
        advance(now)
        guard let shown = outgoing ?? current, now >= shown.showsAt else {
            return nil
        }
        let frame = OverlayFrame(
            phase: shown.phase, phaseElapsed: now - shown.phaseStarted, presence: shown.presence(now),
            handsFree: shown.handsFree, recordingElapsed: (shown.recordingEnded ?? now) - shown.recordingStarted,
            levels: levels, spectrum: spectrum, scale: shown.placement.scale)
        return (frame, shown.placement)
    }

    /// When frame(at:) returns nil: the time a dictation waiting for the show delay appears, if any.
    public var nextShow: Double? { current?.showsAt }

    private func advance(_ now: Double) {
        if let current, let outgoing, now >= current.showsAt {
            current.riseFrom = outgoing.presence(current.showsAt)
            self.outgoing = nil
        }
        if let outgoing, outgoing.gone(now) {
            self.outgoing = nil
        }
        if let current, current.gone(now) {
            self.current = nil
        }
    }
}
