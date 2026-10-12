import AppKit
import DictateCore

/// Pauses whatever macOS shows as Now Playing (music, a video in the browser) while you dictate, and
/// plays it again when the recording stops. It only resumes what it paused, so a dictation never starts music.
///
/// MediaRemote is private. Since macOS 15.4 it answers "is anything playing?" only to Apple's own processes,
/// so that question goes through osascript; pause and play still work from here.
@MainActor
final class MediaPause {
    private typealias SendCommand = @convention(c) (Int32, CFDictionary?) -> Bool
    private static let play: Int32 = 0
    private static let pause: Int32 = 1
    /// The bundle id of the playing app, or an empty line when nothing plays.
    nonisolated private static let query = """
        ObjC.import('Foundation');
        $.NSBundle.bundleWithPath('/System/Library/PrivateFrameworks/MediaRemote.framework').load;
        const request = $.NSClassFromString('MRNowPlayingRequest');
        const path = request.localNowPlayingPlayerPath;
        request.localIsPlaying ? ((path.isNil() ? undefined : ObjC.unwrap(path.client.bundleIdentifier)) || '?') : ''
        """

    private let send: SendCommand?
    /// Bumped by every start and stop, so an answer that arrives after its recording ended is dropped.
    private var generation = 0
    private(set) var paused = false

    init() {
        let handle = dlopen("/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote", RTLD_LAZY)
        send = handle.flatMap { dlsym($0, "MRMediaRemoteSendCommand") }.map { unsafeBitCast($0, to: SendCommand.self) }
        if send == nil {
            Log.warning("MediaRemote is unavailable; media will not pause while recording")
        }
    }

    /// Pauses at the overlay's show delay, once the answer is in, so a shortcut such as Right Command + C
    /// never pauses anything.
    func recordingStarted() {
        generation += 1
        guard send != nil else {
            return
        }
        let current = generation
        let due = DispatchTime.now() + OverlayTiming.showDelay
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let player = Self.playingApp()
            DispatchQueue.main.asyncAfter(deadline: due) {
                MainActor.assumeIsolated { self?.pause(player, generation: current) }
            }
        }
    }

    func recordingStopped() {
        generation += 1
        guard paused, let send else {
            return
        }
        paused = false
        if send(Self.play, nil) {
            Log.info("resumed media")
        } else {
            Log.warning("MediaRemote did not take the play command")
        }
    }

    private func pause(_ player: String?, generation current: Int) {
        guard current == generation, let player, let send else {
            return
        }
        if send(Self.pause, nil) {
            paused = true
            Log.info("paused media in \(player)")
        } else {
            Log.warning("MediaRemote did not take the pause command")
        }
    }

    /// Nil when nothing plays or the question went unanswered within 2 s.
    nonisolated static func playingApp() -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        process.arguments = ["-l", "JavaScript", "-e", query]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            Log.warning("could not ask what is playing: \(error)")
            return nil
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + 2) {
            if process.isRunning {
                process.terminate()
            }
        }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            Log.warning("could not ask what is playing: osascript exited with \(process.terminationStatus)")
            return nil
        }
        let player = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        return player.isEmpty ? nil : player
    }
}
