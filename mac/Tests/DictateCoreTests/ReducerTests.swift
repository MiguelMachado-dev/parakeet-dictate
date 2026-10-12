import Testing

@testable import DictateCore

// A port of tests/test_reducer.py.

let START: [Effect] = [.startRecording, .playSound(.start)]
let STOP: [Effect] = [.stopAndTranscribe, .playSound(.stop)]
let RAW = "um so the meeting is friday"

@discardableResult
func walk(
    _ state: State, _ script: [(Event, State, [Effect])], polish: Bool = false,
    sourceLocation: SourceLocation = #_sourceLocation
) -> State {
    var state = state
    for (i, (event, wantState, wantEffects)) in script.enumerated() {
        let (next, effects) = step(state, event, tapThreshold: 0.3, polish: polish)
        #expect(next == wantState, "step \(i): \(event)", sourceLocation: sourceLocation)
        #expect(effects == wantEffects, "step \(i): \(event)", sourceLocation: sourceLocation)
        state = next
    }
    return state
}

@Test func holdToTalkPastesAfterRelease() {
    walk(.idle(), [
        (.hotkeyDown(10.0), .recording(.hold, pressedAt: 10.0), START),
        (.hotkeyUp(11.5), .transcribing(keyDown: false), STOP),
        (.transcriptReady("olá mundo", requestID: 1), .idle(), [.paste("olá mundo")]),
    ])
}

@Test func releaseExactlyAtThresholdCountsAsHold() {
    walk(.idle(), [
        (.hotkeyDown(0.0), .recording(.hold, pressedAt: 0.0), START),
        (.hotkeyUp(0.3), .transcribing(keyDown: false), STOP),
    ])
}

@Test func tapSwitchesToToggleAndNextPressStops() {
    walk(.idle(), [
        (.hotkeyDown(0.0), .recording(.hold, pressedAt: 0.0), START),
        (.hotkeyUp(0.12), .recording(.toggle, pressedAt: 0.0), []),
        (.hotkeyDown(5.0), .transcribing(keyDown: true), STOP),
        (.hotkeyUp(5.1), .transcribing(keyDown: false), []),
        (.transcriptReady("hands free", requestID: 1), .idle(), [.paste("hands free")]),
    ])
}

@Test func autoRepeatKeydownIsIgnoredWhileHeld() {
    walk(.idle(), [
        (.hotkeyDown(0.0), .recording(.hold, pressedAt: 0.0), START),
        (.hotkeyDown(0.5), .recording(.hold, pressedAt: 0.0), []),
        (.hotkeyDown(0.9), .recording(.hold, pressedAt: 0.0), []),
        (.hotkeyUp(1.0), .transcribing(keyDown: false), STOP),
    ])
}

@Test func autoRepeatAfterCancelDoesNotStartANewRecording() {
    walk(.idle(), [
        (.hotkeyDown(0.0), .recording(.hold, pressedAt: 0.0), START),
        (.cancel, .idle(keyDown: true), [.discardRecording, .playSound(.cancel)]),
        (.hotkeyDown(0.6), .idle(keyDown: true), []),
        (.hotkeyUp(0.8), .idle(), []),
        (.hotkeyDown(2.0), .recording(.hold, pressedAt: 2.0), START),
    ])
}

@Test func escCancelsHandsFreeRecordingWithoutPasting() {
    walk(.idle(), [
        (.hotkeyDown(0.0), .recording(.hold, pressedAt: 0.0), START),
        (.hotkeyUp(0.1), .recording(.toggle, pressedAt: 0.0), []),
        (.cancel, .idle(), [.discardRecording, .playSound(.cancel)]),
        (.transcriptReady("stale", requestID: 1), .idle(), []),
    ])
}

@Test func keysDuringTranscribingOnlyTrackTheHotkey() {
    walk(.transcribing(keyDown: false), [
        (.hotkeyDown(1.0), .transcribing(keyDown: true), []),
        (.hotkeyDown(1.4), .transcribing(keyDown: true), []),
        (.otherKeyDown, .transcribing(keyDown: true), []),
        (.hotkeyUp(1.6), .transcribing(keyDown: false), []),
        (.transcriptReady("ok", requestID: 1), .idle(), [.paste("ok")]),
    ])
}

// The Mac's one addition: a request to the PC can take seconds, so Esc cancels it and pastes nothing.
@Test func escWhileTranscribingCancelsAndIgnoresTheLateTranscript() {
    walk(.transcribing(keyDown: false), [
        (.cancel, .idle(), [.cancelTranscription, .playSound(.cancel)]),
        (.transcriptReady("late", requestID: 1), .idle(), []),
    ])
    walk(.transcribing(keyDown: true), [
        (.cancel, .idle(keyDown: true), [.cancelTranscription, .playSound(.cancel)]),
        (.hotkeyUp(1.0), .idle(), []),
    ])
}

@Test func transcriptArrivingWhileKeyDownPastesOnKeyUp() {
    walk(.recording(.toggle, pressedAt: 0.0), [
        (.hotkeyDown(4.0), .transcribing(keyDown: true), STOP),
        (.transcriptReady("later", requestID: 1), .awaitingRelease(text: "later"), []),
        (.hotkeyDown(4.3), .awaitingRelease(text: "later"), []),
        (.hotkeyUp(4.5), .idle(), [.paste("later")]),
    ])
}

@Test func escWhileAwaitingReleaseDropsTheText() {
    walk(.awaitingRelease(text: "nope"), [
        (.cancel, .idle(keyDown: true), [.playSound(.cancel)]),
        (.hotkeyUp(1.0), .idle(), []),
    ])
}

@Test func emptyTranscriptDoesNotPaste() {
    walk(.transcribing(keyDown: false), [
        (.transcriptReady("", requestID: 1), .idle(), []),
    ])
    walk(.transcribing(keyDown: true), [
        (.transcriptReady("", requestID: 1), .idle(keyDown: true), []),
        (.hotkeyUp(1.0), .idle(), []),
    ])
}

@Test func recordingFailureReturnsToIdle() {
    walk(.idle(), [
        (.hotkeyDown(0.0), .recording(.hold, pressedAt: 0.0), START),
        (.recordingFailed("no mic"), .idle(keyDown: true), [.discardRecording, .playSound(.error)]),
        (.hotkeyUp(0.5), .idle(), []),
        (.hotkeyDown(3.0), .recording(.hold, pressedAt: 3.0), START),
    ])
}

@Test func transcriptionFailureReturnsToIdle() {
    walk(.transcribing(keyDown: false), [
        (.recordingFailed("PC unreachable"), .idle(), [.playSound(.error)]),
    ])
}

@Test func otherKeyWhileHoldingDiscardsAsAShortcut() {
    walk(.idle(), [
        (.hotkeyDown(0.0), .recording(.hold, pressedAt: 0.0), START),
        (.otherKeyDown, .idle(keyDown: true), [.discardRecording]),
        (.hotkeyUp(0.1), .idle(), []),
    ])
}

@Test func typingDuringHandsFreeKeepsRecording() {
    walk(.recording(.toggle, pressedAt: 0.0), [
        (.otherKeyDown, .recording(.toggle, pressedAt: 0.0), []),
    ])
}

@Test func postProcessingPastesThePolishedText() {
    walk(.idle(), [
        (.hotkeyDown(0.0), .recording(.hold, pressedAt: 0.0), START),
        (.hotkeyUp(2.0), .transcribing(keyDown: false), STOP),
        (.transcriptReady(RAW, requestID: 7), .polishing(raw: RAW, requestID: 7, keyDown: false), [.polish(RAW, requestID: 7)]),
        (.polished(requestID: 7, text: "The meeting is Friday."), .idle(), [.paste("The meeting is Friday.")]),
    ], polish: true)
}

@Test func postProcessingOffPastesTheTranscriptDirectly() {
    walk(.transcribing(keyDown: false), [
        (.transcriptReady(RAW, requestID: 7), .idle(), [.paste(RAW)]),
    ], polish: false)
}

@Test func emptyTranscriptSkipsPostProcessing() {
    walk(.transcribing(keyDown: false), [(.transcriptReady("", requestID: 7), .idle(), [])], polish: true)
    walk(.transcribing(keyDown: true), [(.transcriptReady("", requestID: 7), .idle(keyDown: true), [])], polish: true)
}

@Test func polishedTextArrivingWhileKeyDownPastesOnKeyUp() {
    walk(.transcribing(keyDown: true), [
        (.transcriptReady(RAW, requestID: 7), .polishing(raw: RAW, requestID: 7, keyDown: true), [.polish(RAW, requestID: 7)]),
        (.polished(requestID: 7, text: "Friday."), .awaitingRelease(text: "Friday."), []),
        (.hotkeyUp(9.0), .idle(), [.paste("Friday.")]),
    ], polish: true)
}

@Test func hotkeyDuringPolishingOnlyTracksTheKey() {
    walk(.polishing(raw: RAW, requestID: 7, keyDown: false), [
        (.hotkeyDown(1.0), .polishing(raw: RAW, requestID: 7, keyDown: true), []),
        (.hotkeyDown(1.4), .polishing(raw: RAW, requestID: 7, keyDown: true), []),
        (.otherKeyDown, .polishing(raw: RAW, requestID: 7, keyDown: true), []),
        (.hotkeyUp(1.6), .polishing(raw: RAW, requestID: 7, keyDown: false), []),
        (.transcriptReady("ignored", requestID: 8), .polishing(raw: RAW, requestID: 7, keyDown: false), []),
        (.polished(requestID: 7, text: "Friday."), .idle(), [.paste("Friday.")]),
    ], polish: true)
}

@Test func failurePastesTheRawTranscript() {
    walk(.polishing(raw: RAW, requestID: 7, keyDown: false), [
        (.polishFailed(requestID: 7, reason: "timeout"), .idle(), [.paste(RAW)]),
    ])
    walk(.polishing(raw: RAW, requestID: 7, keyDown: true), [
        (.polishFailed(requestID: 7, reason: "HTTP 500"), .awaitingRelease(text: RAW), []),
        (.hotkeyUp(3.0), .idle(), [.paste(RAW)]),
    ])
}

@Test func escWhilePolishingPastesTheRawTranscriptAndIgnoresTheLateReply() {
    walk(.polishing(raw: RAW, requestID: 7, keyDown: false), [
        (.cancel, .idle(), [.paste(RAW)]),
        (.polished(requestID: 7, text: "Friday."), .idle(), []),
        (.polishFailed(requestID: 7, reason: "timeout"), .idle(), []),
    ])
    walk(.polishing(raw: RAW, requestID: 7, keyDown: true), [
        (.cancel, .awaitingRelease(text: RAW), []),
        (.hotkeyUp(3.0), .idle(), [.paste(RAW)]),
    ])
}

@Test func resultForAnotherRequestIsIgnoredWhilePolishing() {
    let second = State.polishing(raw: "second dictation", requestID: 8, keyDown: false)
    walk(second, [
        (.polished(requestID: 7, text: "First."), second, []),
        (.polishFailed(requestID: 7, reason: "timeout"), second, []),
        (.polished(requestID: 8, text: "Second dictation."), .idle(), [.paste("Second dictation.")]),
    ])
}

@Test func lateResultIsIgnoredInEveryOtherState() {
    let states: [State] = [.idle(), .recording(.hold, pressedAt: 0.0), .transcribing(keyDown: false), .awaitingRelease(text: "text")]
    for state in states {
        walk(state, [
            (.polished(requestID: 7, text: "late"), state, []),
            (.polishFailed(requestID: 7, reason: "late"), state, []),
        ], polish: true)
    }
}
