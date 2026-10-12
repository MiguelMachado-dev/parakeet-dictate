import AppKit
import ApplicationServices
import DictateCore
import QuartzCore

/// The visualizer near the bottom of the screen while you dictate (overlay.py). It draws on its own queue
/// at 60 fps while something is on screen; otherwise no timer is armed, so it uses no CPU.
/// Its state is split between its queue and the main thread, as marked below.
final class Overlay: @unchecked Sendable {
    private let queue = DispatchQueue(label: "overlay", qos: .userInteractive)
    private let levels: @Sendable (Double) -> (levels: [Float], spectrum: [Float])
    private let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!

    // Owned by the queue.
    private var scene = OverlayScene()
    private var style: OverlayStyle?
    private var timer: DispatchSourceTimer?
    private var deadline: Double?

    // Owned by the main thread.
    /// Where the next dictation shows.
    @MainActor var position: OverlayPosition
    private let panel: NSPanel
    private let canvas: CALayer
    private var lastPlacement: Placement?

    /// `levels` returns the current recording's meter at a CACurrentMediaTime; the overlay's queue calls it each frame.
    @MainActor
    init(
        style: OverlayStyle?, position: OverlayPosition,
        levels: @escaping @Sendable (Double) -> (levels: [Float], spectrum: [Float])
    ) {
        self.style = style
        self.position = position
        self.levels = levels
        // It must never take focus or clicks, and shows on every Space and over full-screen apps.
        panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: OverlayCanvas.width, height: OverlayCanvas.height),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.isFloatingPanel = true
        panel.level = .statusBar
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.ignoresMouseEvents = true
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        let view = NSView(frame: panel.contentLayoutRect)
        view.wantsLayer = true
        canvas = CALayer()
        canvas.contentsGravity = .resize
        canvas.frame = view.bounds
        canvas.autoresizingMask = [.layerWidthSizable, .layerHeightSizable]
        view.layer?.addSublayer(canvas)
        panel.contentView = view
    }

    /// Called on each dictation transition that changes what the overlay shows.
    @MainActor
    func notify(_ command: OverlayCommand) {
        // Where a dictation shows is decided when its recording starts, on the screen of the focused window.
        if case .show(.recording, _) = command {
            lastPlacement = placementNow()
        }
        let placement = lastPlacement ?? placementNow()
        let t = CACurrentMediaTime()
        queue.async {
            guard self.style != nil else {
                return
            }
            self.scene.apply(command, at: t, placement: placement)
            self.tick()
        }
    }

    /// Draws with `style` from the next frame. Nil turns the overlay off: it hides at once, drops the
    /// dictation on screen, and ignores transitions until a style is set again.
    func restyle(_ style: OverlayStyle?) {
        queue.async {
            self.style = style
            if style == nil {
                self.scene = OverlayScene()
            }
            self.tick()
        }
    }

    private func tick() {
        let now = CACurrentMediaTime()
        let (levels, spectrum) = self.levels(now)
        guard let style, let (frame, placement) = scene.frame(at: now, levels: levels, spectrum: spectrum) else {
            DispatchQueue.main.async { self.hide() }
            deadline = nil
            schedule(at: style == nil ? nil : scene.nextShow)
            return
        }
        let image = cgImage(style.render(frame))
        DispatchQueue.main.async { self.show(image, at: placement) }
        let period = 1 / OverlayCanvas.fps
        if let deadline, deadline + period >= now {
            self.deadline = deadline + period
        } else {
            deadline = now + period
        }
        schedule(at: deadline)
    }

    private func schedule(at due: Double?) {
        timer?.cancel()
        timer = nil
        guard let due else {
            return
        }
        let timer = DispatchSource.makeTimerSource(flags: .strict, queue: queue)
        timer.schedule(deadline: .now() + max(0, due - CACurrentMediaTime()), leeway: .milliseconds(1))
        timer.setEventHandler { [weak self] in self?.tick() }
        timer.resume()
        self.timer = timer
    }

    private func cgImage(_ image: RGBAImage) -> CGImage? {
        guard let provider = CGDataProvider(data: Data(image.pixels) as CFData) else {
            return nil
        }
        return CGImage(
            width: image.width, height: image.height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: image.width * 4,
            space: colorSpace, bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    }

    @MainActor
    private func show(_ image: CGImage?, at placement: Placement) {
        let frame = NSRect(x: placement.x, y: placement.y, width: placement.width, height: placement.height)
        if panel.frame != frame {
            panel.setFrame(frame, display: false)
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        canvas.contentsScale = placement.scale
        canvas.contents = image
        CATransaction.commit()
        if !panel.isVisible {
            Log.info("overlay shown at \(Int(frame.minX)),\(Int(frame.minY)) at \(placement.scale)x")
            panel.orderFrontRegardless()
        }
    }

    @MainActor
    private func hide() {
        if panel.isVisible {
            Log.info("overlay hidden")
            panel.orderOut(nil)
            canvas.contents = nil
        }
    }

    @MainActor
    private func placementNow() -> Placement {
        let screen = Self.screenOfFocusedWindow() ?? Self.screenUnderMouse() ?? NSScreen.main ?? NSScreen.screens[0]
        let visible = screen.visibleFrame
        switch position {
        case .bottom:
            return Placement.bottomCenter(
                visibleX: visible.minX, visibleY: visible.minY, visibleWidth: visible.width, scale: screen.backingScaleFactor)
        case .top:
            return Placement.topCenter(
                visibleX: visible.minX, visibleTop: visible.maxY, visibleWidth: visible.width, scale: screen.backingScaleFactor)
        }
    }

    /// The screen holding the center of the frontmost app's focused window, through the Accessibility API.
    @MainActor
    private static func screenOfFocusedWindow() -> NSScreen? {
        guard let app = NSWorkspace.shared.frontmostApplication else {
            return nil
        }
        let element = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(element, 0.1)
        var window: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXFocusedWindowAttribute as CFString, &window) == .success,
            let window, CFGetTypeID(window) == AXUIElementGetTypeID()
        else {
            return nil
        }
        var positionValue: CFTypeRef?
        var sizeValue: CFTypeRef?
        let focused = window as! AXUIElement
        guard AXUIElementCopyAttributeValue(focused, kAXPositionAttribute as CFString, &positionValue) == .success,
            AXUIElementCopyAttributeValue(focused, kAXSizeAttribute as CFString, &sizeValue) == .success
        else {
            return nil
        }
        var position = CGPoint.zero
        var size = CGSize.zero
        guard AXValueGetValue(positionValue as! AXValue, .cgPoint, &position),
            AXValueGetValue(sizeValue as! AXValue, .cgSize, &size)
        else {
            return nil
        }
        // The Accessibility API measures from the top left of the primary screen; AppKit from its bottom left.
        let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
        let center = NSPoint(x: position.x + size.width / 2, y: primaryHeight - (position.y + size.height / 2))
        return NSScreen.screens.first { $0.frame.contains(center) }
    }

    @MainActor
    private static func screenUnderMouse() -> NSScreen? {
        let mouse = NSEvent.mouseLocation
        return NSScreen.screens.first { $0.frame.contains(mouse) }
    }
}
