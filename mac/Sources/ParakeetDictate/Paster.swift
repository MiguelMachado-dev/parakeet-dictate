import AVFoundation
import AppKit
import DictateCore

/// Pastes through the clipboard and puts back what it held (inject.py `paste`).
@MainActor
final class Paster {
    /// Marks the Cmd+V we post, so our own event tap lets it through.
    nonisolated static let tag: Int64 = 0x5041_524B
    static let restoreDelay: TimeInterval = 0.2
    static let vKey: CGKeyCode = 9
    /// Clipboard managers skip items of this type (nspasteboard.org).
    static let transient = NSPasteboard.PasteboardType("org.nspasteboard.TransientType")

    private typealias Snapshot = [[NSPasteboard.PasteboardType: Data]]
    private var pending: (saved: Snapshot, changeCount: Int)?

    func paste(_ text: String) {
        let pasteboard = NSPasteboard.general
        // A paste that comes before the last one restored must restore the user's clipboard, not our text.
        let saved: Snapshot
        if let pending, pending.changeCount == pasteboard.changeCount {
            saved = pending.saved
        } else {
            saved = Self.snapshot(pasteboard)
        }
        let item = NSPasteboardItem()
        item.setString(text, forType: .string)
        item.setData(Data(), forType: Self.transient)
        pasteboard.clearContents()
        pasteboard.writeObjects([item])
        let ourWrite = pasteboard.changeCount
        pending = (saved, ourWrite)
        Self.postCommandV()

        DispatchQueue.main.asyncAfter(deadline: .now() + Self.restoreDelay) { [weak self] in
            guard let self, self.pending?.changeCount == ourWrite else {
                return
            }
            self.pending = nil
            // Something else wrote to the clipboard meanwhile; leave it.
            guard pasteboard.changeCount == ourWrite else {
                return
            }
            Self.restore(saved, to: pasteboard)
        }
    }

    private static func snapshot(_ pasteboard: NSPasteboard) -> Snapshot {
        (pasteboard.pasteboardItems ?? []).map { item in
            var types: [NSPasteboard.PasteboardType: Data] = [:]
            for type in item.types {
                if let data = item.data(forType: type) {
                    types[type] = data
                }
            }
            return types
        }
    }

    private static func restore(_ saved: Snapshot, to pasteboard: NSPasteboard) {
        pasteboard.clearContents()
        let items = saved.filter { !$0.isEmpty }.map { types -> NSPasteboardItem in
            let item = NSPasteboardItem()
            for (type, data) in types {
                item.setData(data, forType: type)
            }
            item.setData(Data(), forType: transient)
            return item
        }
        if !items.isEmpty {
            pasteboard.writeObjects(items)
        }
    }

    private static func postCommandV() {
        let source = CGEventSource(stateID: .privateState)
        for down in [true, false] {
            let event = CGEvent(keyboardEventSource: source, virtualKey: vKey, keyDown: down)
            event?.flags = .maskCommand
            event?.setIntegerValueField(.eventSourceUserData, value: tag)
            event?.post(tap: .cghidEventTap)
        }
    }
}

/// The start, stop, cancel, and error tones. They play on their own queue: right after the microphone stops,
/// starting a sound on AirPods took up to 733 ms, and on the main thread the transcription waited for it.
final class Cues: @unchecked Sendable {
    private let queue = DispatchQueue(label: "cues", qos: .userInitiated)
    // Only touched on the queue.
    private var players: [Cue: AVAudioPlayer] = [:]

    init() {
        queue.async {
            for cue in Cue.allCases {
                self.players[cue] = try? AVAudioPlayer(data: Tones.wav(cue))
            }
        }
    }

    func play(_ cue: Cue) {
        queue.async {
            guard let player = self.players[cue] else {
                return
            }
            let started = ProcessInfo.processInfo.systemUptime
            player.stop()
            player.currentTime = 0
            player.play()
            let ms = Int((ProcessInfo.processInfo.systemUptime - started) * 1000)
            if ms >= 20 {
                Log.warning("the \(cue.rawValue) sound took \(ms) ms to start")
            }
        }
    }
}
