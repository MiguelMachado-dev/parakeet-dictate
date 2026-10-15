import AppKit
import DictateCore

/// What the menu shows about the PC.
enum ServerStatus: Equatable {
    case notConfigured
    case unknown
    case connected(postprocess: Bool)
    case failed(ServerError)
}

/// Runs the reducer and its effects on the main thread (app.py `Dictation`).
@MainActor
final class Dictation {
    private let settings: Settings
    let recorder = Recorder()
    private let cues = Cues()
    private let paster = Paster()
    let media = MediaPause()
    let local = LocalEngine()
    private(set) var tap: HotkeyTap!

    private(set) var state: State = .idle() {
        didSet { stateChanged(from: oldValue) }
    }
    private var server: ServerClient?
    private var polisher: (any Polisher)?
    private var transcription: Task<Void, Never>?
    /// Bumped by every new or cancelled transcription, so a stale result is dropped.
    private var generation = 0
    private var nextRequestID = 1
    private var releasedAt = 0.0
    private var releasePoll: Timer?
    /// What the rules for the app in front ask of the model, since the text goes to that app. "" for no rules.
    private var instructions = ""

    var onStateChange: ((State) -> Void)?
    /// What the overlay should show after a transition, if it changes.
    var onOverlayCommand: ((OverlayCommand) -> Void)?
    var onLocalStatus: ((LocalEngine.Status) -> Void)?
    var onServerStatus: ((ServerStatus) -> Void)?
    private(set) var localStatus: LocalEngine.Status = .notLoaded
    private(set) var serverStatus: ServerStatus = .notConfigured {
        didSet {
            if serverStatus != oldValue {
                onServerStatus?(serverStatus)
            }
        }
    }

    init(settings: Settings) {
        self.settings = settings
        tap = HotkeyTap(hotkey: settings.hotkey) { [weak self] event in
            DispatchQueue.main.async { self?.post(event) }
        }
        let updates = local.statusUpdates
        Task { [weak self] in
            for await status in updates {
                self?.localStatus = status
                self?.onLocalStatus?(status)
            }
        }
        applySettings()
    }

    /// Rebuilds the clients and loads or unloads the local model after a settings change.
    func applySettings() {
        tap.hotkey = settings.hotkey
        if let connection = settings.serverConnection {
            if server?.connection != connection {
                server = ServerClient(connection: connection)
                serverStatus = .unknown
            }
        } else {
            server = nil
            serverStatus = .notConfigured
        }
        polisher = settings.makePolisher()
        switch settings.engine {
        case .local:
            Task { await local.load() }
        case .server:
            Task { await local.unload() }
            checkServer()
        }
    }

    /// Updates the PC's state in the menu. The PC also warms its post-processing connection.
    func checkServer() {
        guard settings.engine == .server, let server else {
            return
        }
        Task {
            do {
                let postprocess = try await server.health()
                if server === self.server {
                    serverStatus = .connected(postprocess: postprocess)
                }
            } catch let error as ServerError {
                if server === self.server {
                    serverStatus = .failed(error)
                }
            } catch {}
        }
    }

    /// A rule post-processes even with post-processing off.
    private var postprocess: Bool {
        settings.postprocess || !instructions.isEmpty
    }

    private var polishOn: Bool {
        settings.engine == .local && postprocess && polisher != nil
    }

    /// Reads the rules for the app in front when recording starts, to warm the model, and again when it stops,
    /// since a hands-free dictation can switch apps meanwhile.
    private func readRules(log: Bool) {
        let app = NSWorkspace.shared.frontmostApplication
        let rules = settings.rules.matching(app?.bundleIdentifier)
        instructions = combinedInstructions(rules)
        guard log, !rules.isEmpty else {
            return
        }
        Log.info("rules for \(app?.localizedName ?? "the app in front"): \(rules.map(\.name).joined(separator: ", "))")
        if settings.engine == .local && polisher == nil {
            Log.warning("the rules need a \(settings.polishProvider.keyName) API key in Settings; pasting the raw transcript")
        }
    }

    func post(_ event: Event) {
        let began = ProcessInfo.processInfo.systemUptime
        defer {
            let ms = Int((ProcessInfo.processInfo.systemUptime - began) * 1000)
            if ms >= 50 {
                Log.warning("handling \(describe(event)) took \(ms) ms on the main thread")
            }
        }
        if case .hotkeyUp(let t) = event {
            releasedAt = t
        }
        // The tap runs on its own thread, so a late event means the main thread was busy.
        if case .hotkeyDown(let t) = event, let late = Self.lateMs(since: t) {
            Log.warning("HotkeyDown reached the main thread \(late) ms after the tap saw it")
        }
        if case .hotkeyUp(let t) = event, let late = Self.lateMs(since: t) {
            Log.warning("HotkeyUp reached the main thread \(late) ms after the tap saw it")
        }
        let previous = state
        let (next, effects) = step(previous, event, tapThreshold: 0.3, polish: polishOn)
        if next != previous {
            Log.info("\(describe(previous)) -> \(describe(next)) on \(describe(event))")
            if case .polishFailed(_, let reason) = event {
                Log.warning("post-processing failed (\(reason)); pasting the raw transcript")
            }
        }
        state = next
        if next != previous, var command = overlayCommand(previous: previous, state: next, event: event, effects: effects) {
            // The Mac cannot tell the PC's transcription from its post-processing, so the whole request
            // gets the post-processing look when it is asked for.
            if command == .show(.transcribing), settings.engine == .server, postprocess {
                command = .show(.polishing)
            }
            onOverlayCommand?(command)
        }
        for effect in effects {
            execute(effect)
        }
    }

    private static func lateMs(since t: Double) -> Int? {
        let late = Int((ProcessInfo.processInfo.systemUptime - t) * 1000)
        return late >= 50 ? late : nil
    }

    /// After the current step's effects, like an event queued on the Windows controller thread.
    private func postLater(_ event: Event) {
        DispatchQueue.main.async { [weak self] in self?.post(event) }
    }

    private func execute(_ effect: Effect) {
        switch effect {
        case .startRecording:
            readRules(log: false)
            do {
                try recorder.start()
                Log.info("recording from \(recorder.deviceName), started in \(recorder.startLaps)")
            } catch {
                Log.error("microphone did not start: \(error)")
                postLater(.recordingFailed("\(error)"))
                return
            }
            if settings.engine == .server {
                checkServer()
            } else if polishOn, let polisher {
                Task { await polisher.preconnect() }
            }
        case .stopAndTranscribe:
            readRules(log: true)
            do {
                transcribe(try recorder.stop())
            } catch {
                Log.error("microphone did not stop cleanly: \(error)")
                postLater(.recordingFailed("\(error)"))
            }
        case .discardRecording:
            recorder.discard()
        case .cancelTranscription:
            generation += 1
            transcription?.cancel()
            transcription = nil
        case .paste(let text):
            let sinceRelease = Int((ProcessInfo.processInfo.systemUptime - releasedAt) * 1000)
            paster.paste(settings.appendSpace ? text + " " : text)
            Log.info("pasted \(text.count) chars, sent \(sinceRelease) ms after the hotkey release")
        case .playSound(let cue):
            if settings.sounds {
                cues.play(cue)
            }
        case .polish(let text, let requestID):
            polish(text, requestID: requestID)
        }
    }

    private func transcribe(_ samples: [Float]) {
        generation += 1
        let current = generation
        let engine = settings.engine
        let server = server
        let postprocess = self.postprocess
        let instructions = self.instructions
        let started = Date()
        transcription = Task {
            // The task starts on the main thread, so this is how long the main thread kept it waiting.
            let waited = Int(Date().timeIntervalSince(started) * 1000)
            do {
                let text: String
                switch engine {
                case .local:
                    text = try await local.transcribe(samples)
                case .server:
                    text = try await self.transcribeOnServer(
                        samples, server: server, postprocess: postprocess, instructions: instructions)
                }
                guard current == generation else {
                    return
                }
                let seconds = Double(samples.count) / Resampler.modelRate
                Log.info(
                    "transcribed \(String(format: "%.1f", seconds)) s on \(engine.title) in "
                        + "\(Int(Date().timeIntervalSince(started) * 1000)) ms (started after \(waited) ms): \(text.count) chars")
                post(.transcriptReady(text, requestID: nextRequestID))
                nextRequestID += 1
            } catch is CancellationError {
            } catch {
                guard current == generation else {
                    return
                }
                Log.error("transcription failed: \(error)")
                post(.recordingFailed("\(error)"))
            }
        }
    }

    private func transcribeOnServer(
        _ samples: [Float], server: ServerClient?, postprocess: Bool, instructions: String
    ) async throws -> String {
        guard let server else {
            throw ServerError.unreachable("no PC address and token in Settings")
        }
        if Double(samples.count) < LocalEngine.minSeconds * Resampler.modelRate {
            return ""
        }
        do {
            let wav = Wav.encode(samples, rate: Int(Resampler.modelRate))
            let reply = try await server.transcribe(wav: wav, postprocess: postprocess, instructions: instructions)
            if server === self.server, case .failed = serverStatus {
                checkServer()
            }
            if postprocess && !reply.polished && !reply.text.isEmpty {
                Log.warning("the PC returned the raw transcript: its post-processor failed or has no key")
            }
            return reply.text
        } catch let error as ServerError {
            if server === self.server {
                serverStatus = .failed(error)
            }
            throw error
        }
    }

    private func polish(_ text: String, requestID: Int) {
        guard let polisher else {
            post(.polishFailed(requestID: requestID, reason: "no \(settings.polishProvider.keyName) API key"))
            return
        }
        let instructions = self.instructions
        Task {
            do {
                post(.polished(requestID: requestID, text: try await polisher.polish(text, instructions: instructions)))
            } catch {
                post(.polishFailed(requestID: requestID, reason: "\(error)"))
            }
        }
        // URLSession's timeout is per read, not for the whole request, so this bounds the wait. When the
        // reply comes first, the reducer ignores this failure because its id is no longer current.
        let timeout = settings.polishTimeout
        DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { [weak self] in
            self?.post(.polishFailed(requestID: requestID, reason: "no reply within \(Int(timeout)) s"))
        }
    }

    private func stateChanged(from previous: State) {
        switch state {
        case .transcribing, .polishing:
            tap.busy = true
        default:
            tap.busy = false
        }
        switch (previous, state) {
        case (.recording, .recording):
            break
        case (_, .recording):
            if settings.pauseMedia {
                media.recordingStarted()
            }
        case (.recording, _):
            media.recordingStopped()
        default:
            break
        }
        // The tap can miss a release, for example while macOS shows a permission prompt. While a finished
        // dictation waits for the release, check the key itself (app.py polls GetAsyncKeyState the same way).
        if case .awaitingRelease = state {
            if releasePoll == nil {
                releasePoll = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
                    MainActor.assumeIsolated { self?.checkRelease() }
                }
            }
        } else {
            releasePoll?.invalidate()
            releasePoll = nil
        }
        onStateChange?(state)
    }

    private func checkRelease() {
        let hotkey = settings.hotkey
        let down: Bool
        if let mask = hotkey.modifierMask {
            down = CGEventSource.flagsState(.combinedSessionState).rawValue & mask != 0
        } else {
            down = CGEventSource.keyState(.combinedSessionState, key: CGKeyCode(hotkey.rawValue))
        }
        if !down {
            Log.warning("hotkey release was not seen by the tap; using the key state")
            tap.hotkeyHeld = false
            post(.hotkeyUp(ProcessInfo.processInfo.systemUptime))
        }
    }
}

/// Like the Python reprs, these never include transcript text.
private func describe(_ state: State) -> String {
    switch state {
    case .idle(let keyDown): return "Idle(keyDown: \(keyDown))"
    case .recording(let mode, _): return "Recording(\(mode))"
    case .transcribing(let keyDown): return "Transcribing(keyDown: \(keyDown))"
    case .polishing(_, let id, let keyDown): return "Polishing(#\(id), keyDown: \(keyDown))"
    case .awaitingRelease: return "AwaitingRelease"
    }
}

private func describe(_ event: Event) -> String {
    switch event {
    case .hotkeyDown: return "HotkeyDown"
    case .hotkeyUp: return "HotkeyUp"
    case .otherKeyDown: return "OtherKeyDown"
    case .cancel: return "Cancel"
    case .transcriptReady(let text, let id): return "TranscriptReady(#\(id), \(text.count) chars)"
    case .recordingFailed(let error): return "RecordingFailed(\(error))"
    case .polished(let id, let text): return "Polished(#\(id), \(text.count) chars)"
    case .polishFailed(let id, _): return "PolishFailed(#\(id))"
    }
}
