import Foundation
import Testing

@testable import DictateCore

// A port of tests/test_overlay_scene.py.

private let here = Placement.bottomCenter(visibleX: 0, visibleY: 0, visibleWidth: 2560, scale: 2)
private let silentLevels = [Float](repeating: 0, count: LevelMeter.levelHistory)
private let silentSpectrum = [Float](repeating: 0, count: LevelMeter.spectrumBands)
private let beatS = OverlayCanvas.beat
private let delay = OverlayTiming.showDelay

/// The overlay command for each state change the reducer makes, as Dictation reports them.
private func commands(_ events: [Event], polish: Bool = false) -> [OverlayCommand?] {
    var state = State.idle()
    var out: [OverlayCommand?] = []
    for event in events {
        let previous = state
        let (next, effects) = step(state, event, tapThreshold: 0.3, polish: polish)
        state = next
        if state != previous {
            out.append(overlayCommand(previous: previous, state: state, event: event, effects: effects))
        }
    }
    return out
}

private func frame(_ scene: OverlayScene, _ t: Double) -> OverlayFrame? {
    scene.frame(at: t, levels: silentLevels, spectrum: silentSpectrum)?.0
}

private func ticks(_ start: Double, _ end: Double) -> [Double] {
    Array(stride(from: start, to: end, by: 1 / OverlayCanvas.fps))
}

private func close(_ a: Double, _ b: Double) -> Bool { abs(a - b) < 1e-6 }

@Test func holdToTalkRecordsThenTranscribesThenPlaysDone() {
    #expect(commands([.hotkeyDown(0), .hotkeyUp(2), .transcriptReady("hi", requestID: 1)])
        == [.show(.recording), .show(.transcribing), .finish(beat: .done)])
}

@Test func aTapTurnsTheRecordingHandsFree() {
    #expect(commands([.hotkeyDown(0), .hotkeyUp(0.1), .hotkeyDown(3), .hotkeyUp(3.1), .transcriptReady("hi", requestID: 1)])
        == [.show(.recording), .show(.recording, handsFree: true), .show(.transcribing), .show(.transcribing), .finish(beat: .done)])
}

@Test func polishingAndAwaitingRelease() {
    #expect(commands([.hotkeyDown(0), .hotkeyUp(2), .transcriptReady("um hi", requestID: 1), .polished(requestID: 1, text: "Hi.")], polish: true)
        == [.show(.recording), .show(.transcribing), .show(.polishing), .finish(beat: .done)])
    #expect(commands([.hotkeyDown(0), .hotkeyUp(0.1), .hotkeyDown(3), .transcriptReady("hi", requestID: 1), .hotkeyUp(4)])
        == [.show(.recording), .show(.recording, handsFree: true), .show(.transcribing), .show(.transcribing), .finish(beat: .done)])
}

@Test func escWhilePolishingPastesTheRawTextSoItPlaysDone() {
    #expect(commands([.hotkeyDown(0), .hotkeyUp(2), .transcriptReady("um hi", requestID: 1), .cancel], polish: true).last == .finish(beat: .done))
}

@Test func cancelPlaysCancelled() {
    #expect(commands([.hotkeyDown(0), .cancel, .hotkeyUp(1)]) == [.show(.recording), .finish(beat: .cancelled), nil])
    let awaiting: [Event] = [.hotkeyDown(0), .hotkeyUp(0.1), .hotkeyDown(3), .transcriptReady("hi", requestID: 1), .cancel, .hotkeyUp(4)]
    #expect(Array(commands(awaiting).suffix(2)) == [.finish(beat: .cancelled), nil])
}

// The Mac's Esc during a request to the PC.
@Test func escWhileTranscribingPlaysCancelled() {
    #expect(commands([.hotkeyDown(0), .hotkeyUp(2), .cancel]) == [.show(.recording), .show(.transcribing), .finish(beat: .cancelled)])
}

@Test func failuresPlayFailed() {
    #expect(commands([.hotkeyDown(0), .recordingFailed("no mic")]) == [.show(.recording), .finish(beat: .failed)])
    #expect(commands([.hotkeyDown(0), .hotkeyUp(2), .recordingFailed("PC")]) == [.show(.recording), .show(.transcribing), .finish(beat: .failed)])
}

@Test func aShortcutVanishesWithNoBeat() {
    #expect(commands([.hotkeyDown(0), .otherKeyDown, .hotkeyUp(0.2)]) == [.show(.recording), .vanish, nil])
}

@Test func anEmptyTranscriptFadesWithNoBeat() {
    #expect(commands([.hotkeyDown(0), .hotkeyUp(2), .transcriptReady("", requestID: 1)]) == [.show(.recording), .show(.transcribing), .finish(beat: nil)])
}

@Test func aShortcutIsNeverVisible() {
    let scene = OverlayScene()
    scene.apply(.show(.recording), at: 0, placement: here)
    #expect(ticks(0, 0.12).allSatisfy { frame(scene, $0) == nil })
    scene.apply(.vanish, at: 0.12, placement: here)
    #expect(ticks(0.12, 2).allSatisfy { frame(scene, $0) == nil })
    #expect(scene.nextShow == nil)
}

@Test func escDuringTheShowDelayIsNeverVisible() {
    let scene = OverlayScene()
    scene.apply(.show(.recording), at: 0, placement: here)
    scene.apply(.finish(beat: .cancelled), at: 0.1, placement: here)
    #expect(ticks(0, 2).allSatisfy { frame(scene, $0) == nil })
}

@Test func recordingAppearsAfterTheDelayAndFadesIn() throws {
    let scene = OverlayScene()
    scene.apply(.show(.recording), at: 10, placement: here)
    #expect(close(try #require(scene.nextShow), 10 + delay))
    #expect(frame(scene, 10 + delay - 0.001) == nil)
    let half = try #require(frame(scene, 10 + delay + 0.075))
    #expect(half.phase == .recording && close(half.presence, 0.5))
    #expect(close(half.phaseElapsed, 0.075))
    #expect(close(half.recordingElapsed, delay + 0.075))
    #expect(frame(scene, 10 + delay + 0.15)?.presence == 1)
    #expect(scene.frame(at: 11, levels: silentLevels, spectrum: silentSpectrum)?.1 == here)
}

@Test(arguments: [Phase.done, .cancelled, .failed])
func aBeatPlaysThenFadesOut(beat: Phase) throws {
    let scene = OverlayScene()
    scene.apply(.show(.recording), at: 0, placement: here)
    _ = frame(scene, 1)
    scene.apply(.finish(beat: beat), at: 2, placement: here)
    let playing = try #require(frame(scene, 2 + beatS - 0.01))
    #expect(playing.phase == beat && playing.presence == 1 && close(playing.phaseElapsed, beatS - 0.01))
    let fading = try #require(frame(scene, 2 + beatS + 0.1))
    #expect(fading.phase == beat && close(fading.presence, 0.5))
    #expect(frame(scene, 2 + beatS + 0.2 + 0.001) == nil)
    #expect(scene.nextShow == nil)
}

@Test func noBeatFadesOutAtOnceInTheCurrentPhase() throws {
    let scene = OverlayScene()
    scene.apply(.show(.recording), at: 0, placement: here)
    scene.apply(.show(.transcribing), at: 1, placement: here)
    scene.apply(.finish(beat: nil), at: 1.5, placement: here)
    let fading = try #require(frame(scene, 1.5 + OverlayTiming.fadeOut / 4))
    #expect(fading.phase == .transcribing && close(fading.presence, 0.75))
    #expect(frame(scene, 1.5 + OverlayTiming.fadeOut + 0.001) == nil)
}

@Test func aShortcutAfterTheWindowAppearedHidesItAtOnce() {
    let scene = OverlayScene()
    scene.apply(.show(.recording), at: 0, placement: here)
    #expect(frame(scene, 0.6)?.presence == 1)
    scene.apply(.vanish, at: 0.7, placement: here)
    #expect(frame(scene, 0.7) == nil)
}

@Test func handsFreeSticksAndRecordingTimeFreezesWhenRecordingStops() throws {
    let scene = OverlayScene()
    scene.apply(.show(.recording), at: 0, placement: here)
    _ = frame(scene, delay + 0.01)
    scene.apply(.show(.recording, handsFree: true), at: delay + 0.02, placement: here)
    let recording = try #require(frame(scene, 1))
    #expect(recording.handsFree && close(recording.phaseElapsed, 1 - delay))
    scene.apply(.show(.transcribing), at: 2, placement: here)
    let transcribing = try #require(frame(scene, 2.5))
    #expect(transcribing.phase == .transcribing && transcribing.handsFree)
    #expect(close(transcribing.phaseElapsed, 0.5))
    #expect(close(transcribing.recordingElapsed, 2))
}

@Test func aNewRecordingDuringAFadeOutRisesFromWhereTheFadeWas() throws {
    let scene = OverlayScene()
    let elsewhere = Placement.bottomCenter(visibleX: -2560, visibleY: 0, visibleWidth: 2560, scale: 1)
    scene.apply(.show(.recording), at: 0, placement: here)
    scene.apply(.finish(beat: .done), at: 1, placement: here)
    scene.apply(.show(.recording), at: 1.3, placement: elsewhere)
    #expect(frame(scene, 1.3 + delay - 0.01)?.phase == .done)
    let fadeProgress = (1.3 + delay - (1 + beatS)) / OverlayTiming.fadeOut
    let (new, place) = try #require(scene.frame(at: 1.3 + delay, levels: silentLevels, spectrum: silentSpectrum))
    #expect(new.phase == .recording && close(new.presence, 1 - fadeProgress) && place == elsewhere)
    #expect(close(try #require(frame(scene, 1.3 + delay + OverlayTiming.fadeIn * fadeProgress)).presence, 1))
}

@Test func aShortcutDuringThePreviousBeatLeavesTheBeatAlone() throws {
    let scene = OverlayScene()
    scene.apply(.show(.recording), at: 0, placement: here)
    scene.apply(.finish(beat: .done), at: 1, placement: here)
    scene.apply(.show(.recording), at: 1.1, placement: here)
    scene.apply(.vanish, at: 1.2, placement: here)
    let playing = try #require(frame(scene, 1.3))
    #expect(playing.phase == .done && playing.presence == 1)
    #expect(frame(scene, 1 + beatS + OverlayTiming.fadeOut + 0.001) == nil)
    #expect(scene.nextShow == nil)
}

@Test func placementIsBottomCenterOfTheVisibleFrame() {
    #expect(Placement.bottomCenter(visibleX: -1512, visibleY: 30, visibleWidth: 1512, scale: 2)
        == Placement(x: -1512 + 556, y: 30 + 48, width: 400, height: 120, scale: 2))
}

@Test func placementAtTheTopSitsUnderTheMenuBar() {
    // A 1512 x 982 screen whose menu bar takes 37 points, so its visible frame ends at 945.
    #expect(Placement.topCenter(visibleX: 0, visibleTop: 945, visibleWidth: 1512, scale: 2)
        == Placement(x: 556, y: 945 - 16 - 120, width: 400, height: 120, scale: 2))
}
