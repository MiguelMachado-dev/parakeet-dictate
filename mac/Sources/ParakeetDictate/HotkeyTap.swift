import CoreGraphics
import DictateCore
import Foundation

/// The global keyboard hook, an active event tap so it can swallow Esc (app.py `_on_key`).
/// It runs on its own thread, so a busy main thread never delays the user's typing, and only
/// posts events: the controller handles them on the main thread.
final class HotkeyTap {
    static let escape: Int64 = 53

    private let post: (Event) -> Void
    private let lock = NSLock()
    private var _hotkey: Hotkey
    private var _busy = false
    private var _hotkeyHeld = false
    // Only touched on the tap thread.
    private var escSwallowed = false
    private var port: CFMachPort?

    init(hotkey: Hotkey, post: @escaping (Event) -> Void) {
        _hotkey = hotkey
        self.post = post
    }

    var hotkey: Hotkey {
        get { lock.withLock { _hotkey } }
        set { lock.withLock { _hotkey = newValue; _hotkeyHeld = false } }
    }

    /// Set while a transcription or post-processing is in flight, when Esc belongs to the app.
    var busy: Bool {
        get { lock.withLock { _busy } }
        set { lock.withLock { _busy = newValue } }
    }

    var hotkeyHeld: Bool {
        get { lock.withLock { _hotkeyHeld } }
        set { lock.withLock { _hotkeyHeld = newValue } }
    }

    var running: Bool { port != nil }

    /// Fails without the Accessibility permission.
    func start() -> Bool {
        if port != nil {
            return true
        }
        let types: [CGEventType] = [.keyDown, .keyUp, .flagsChanged, .leftMouseDown, .rightMouseDown, .otherMouseDown]
        let mask = types.reduce(CGEventMask(0)) { $0 | (1 << $1.rawValue) }
        guard
            let port = CGEvent.tapCreate(
                tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap, eventsOfInterest: mask,
                callback: { _, type, event, info in
                    let tap = Unmanaged<HotkeyTap>.fromOpaque(info!).takeUnretainedValue()
                    return tap.handle(type, event) ? nil : Unmanaged.passUnretained(event)
                },
                userInfo: Unmanaged.passUnretained(self).toOpaque())
        else {
            return false
        }
        self.port = port
        let source = CFMachPortCreateRunLoopSource(nil, port, 0)
        let thread = Thread {
            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .commonModes)
            CFRunLoopRun()
        }
        thread.name = "hotkey tap"
        thread.qualityOfService = .userInteractive
        thread.start()
        return true
    }

    /// Returns whether to swallow the event. Must return fast: macOS disables a slow tap.
    private func handle(_ type: CGEventType, _ event: CGEvent) -> Bool {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            Log.warning("event tap was disabled (\(type.rawValue)); enabling it again")
            if let port {
                CGEvent.tapEnable(tap: port, enable: true)
            }
            return false
        }
        if event.getIntegerValueField(.eventSourceUserData) == Paster.tag {
            return false
        }
        let now = ProcessInfo.processInfo.systemUptime
        let keyCode = event.getIntegerValueField(.keyboardEventKeycode)
        let hotkey = self.hotkey
        switch type {
        case .flagsChanged:
            if let mask = hotkey.modifierMask, keyCode == hotkey.keyCode {
                let down = event.flags.rawValue & mask != 0
                hotkeyHeld = down
                post(down ? .hotkeyDown(now) : .hotkeyUp(now))
            }
            return false
        case .keyDown, .keyUp:
            let down = type == .keyDown
            if hotkey.modifierMask == nil, keyCode == hotkey.keyCode {
                hotkeyHeld = down
                post(down ? .hotkeyDown(now) : .hotkeyUp(now))
                return false
            }
            if keyCode == Self.escape {
                if down {
                    // While the text is on its way, Esc cancels it here and must not also act in the focused app.
                    escSwallowed = hotkeyHeld || busy
                    post(.cancel)
                }
                return escSwallowed
            }
            if down {
                post(.otherKeyDown)
            }
            return false
        default:
            // A click with the hotkey held is a shortcut too, such as Command-click.
            if hotkeyHeld {
                post(.otherKeyDown)
            }
            return false
        }
    }
}
