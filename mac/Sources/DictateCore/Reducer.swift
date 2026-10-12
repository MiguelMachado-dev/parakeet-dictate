// A port of src/parakeet_dictate/reducer.py. The one difference: Esc while transcribing cancels,
// because a request to the PC can take seconds, while the Windows app only ever waits for its own GPU.

public enum Mode: Equatable, Sendable {
    case hold
    case toggle
}

public enum Cue: String, CaseIterable, Sendable {
    case start
    case stop
    case cancel
    case error
}

public enum State: Equatable, Sendable {
    case idle(keyDown: Bool = false)
    case recording(Mode, pressedAt: Double)
    case transcribing(keyDown: Bool)
    case polishing(raw: String, requestID: Int, keyDown: Bool)
    case awaitingRelease(text: String)

    public var keyDown: Bool {
        switch self {
        case .idle(let keyDown), .transcribing(let keyDown), .polishing(_, _, let keyDown):
            return keyDown
        case .recording(let mode, _):
            return mode == .hold
        case .awaitingRelease:
            return true
        }
    }
}

public enum Event: Equatable, Sendable {
    case hotkeyDown(Double)
    case hotkeyUp(Double)
    case otherKeyDown
    case cancel
    case transcriptReady(String, requestID: Int)
    case recordingFailed(String)
    case polished(requestID: Int, text: String)
    case polishFailed(requestID: Int, reason: String)
}

public enum Effect: Equatable, Sendable {
    case startRecording
    case stopAndTranscribe
    case discardRecording
    case cancelTranscription
    case paste(String)
    case playSound(Cue)
    case polish(String, requestID: Int)
}

public func step(
    _ state: State, _ event: Event, tapThreshold: Double = 0.3, polish: Bool = false
) -> (State, [Effect]) {
    switch (state, event) {
    case (.idle(keyDown: false), .hotkeyDown(let t)):
        return (.recording(.hold, pressedAt: t), [.startRecording, .playSound(.start)])
    case (.idle, .hotkeyUp):
        return (.idle(), [])

    case (.recording(.hold, let pressedAt), .hotkeyUp(let t)):
        if t - pressedAt < tapThreshold {
            return (.recording(.toggle, pressedAt: pressedAt), [])
        }
        return (.transcribing(keyDown: false), [.stopAndTranscribe, .playSound(.stop)])
    case (.recording(.toggle, _), .hotkeyDown):
        return (.transcribing(keyDown: true), [.stopAndTranscribe, .playSound(.stop)])
    case (.recording(.hold, _), .otherKeyDown):
        return (.idle(keyDown: true), [.discardRecording])
    case (.recording, .cancel):
        return (.idle(keyDown: state.keyDown), [.discardRecording, .playSound(.cancel)])
    case (.recording, .recordingFailed):
        return (.idle(keyDown: state.keyDown), [.discardRecording, .playSound(.error)])

    case (.transcribing, .hotkeyDown):
        return (.transcribing(keyDown: true), [])
    case (.transcribing, .hotkeyUp):
        return (.transcribing(keyDown: false), [])
    case (.polishing(let raw, let requestID, _), .hotkeyDown):
        return (.polishing(raw: raw, requestID: requestID, keyDown: true), [])
    case (.polishing(let raw, let requestID, _), .hotkeyUp):
        return (.polishing(raw: raw, requestID: requestID, keyDown: false), [])
    case (.transcribing(let keyDown), .cancel):
        return (.idle(keyDown: keyDown), [.cancelTranscription, .playSound(.cancel)])
    case (.transcribing(let keyDown), .transcriptReady(let text, let requestID)):
        if text.isEmpty {
            return (.idle(keyDown: keyDown), [])
        }
        if polish {
            return (.polishing(raw: text, requestID: requestID, keyDown: keyDown), [.polish(text, requestID: requestID)])
        }
        return deliver(text, keyDown: keyDown)
    case (.transcribing(let keyDown), .recordingFailed):
        return (.idle(keyDown: keyDown), [.playSound(.error)])

    // A result whose id is not the current request belongs to a dictation that was
    // already pasted; it falls through and is ignored.
    case (.polishing(_, let current, let keyDown), .polished(let requestID, let text)) where requestID == current:
        return deliver(text, keyDown: keyDown)
    case (.polishing(let raw, let current, let keyDown), .polishFailed(let requestID, _)) where requestID == current:
        return deliver(raw, keyDown: keyDown)
    case (.polishing(let raw, _, let keyDown), .cancel):
        return deliver(raw, keyDown: keyDown)

    case (.awaitingRelease(let text), .hotkeyUp):
        return (.idle(), [.paste(text)])
    case (.awaitingRelease, .cancel):
        return (.idle(keyDown: true), [.playSound(.cancel)])

    default:
        return (state, [])
    }
}

private func deliver(_ text: String, keyDown: Bool) -> (State, [Effect]) {
    if keyDown {
        return (.awaitingRelease(text: text), [])
    }
    return (.idle(), [.paste(text)])
}
