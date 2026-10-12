import AVFoundation
import AppKit
import DictateCore
import ServiceManagement
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    private let settings = Settings()
    private var dictation: Dictation!
    private var overlay: Overlay!
    private var statusItem: NSStatusItem!
    private var settingsWindow: NSWindow?
    private var permissionPoll: Timer?
    private var healthTimer: Timer?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let bundleID = Bundle.main.bundleIdentifier ?? ""
        if !bundleID.isEmpty, NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).count > 1 {
            Log.info("already running; quitting this copy")
            NSApp.terminate(nil)
            return
        }
        Log.info("starting \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") ?? "dev") on \(settings.engine.title)")

        installMainMenu()
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem.menu = NSMenu()
        statusItem.menu?.delegate = self

        dictation = Dictation(settings: settings)
        let recorder = dictation.recorder
        overlay = Overlay(style: settings.overlay, position: settings.overlayPosition) { recorder.levels() }
        dictation.onOverlayCommand = { [weak self] command in self?.overlay.notify(command) }
        dictation.onStateChange = { [weak self] _ in self?.updateIcon() }
        dictation.onLocalStatus = { [weak self] _ in self?.updateIcon() }
        dictation.onServerStatus = { [weak self] _ in self?.updateIcon() }
        settings.onChange = { [weak self] in
            guard let self else {
                return
            }
            self.dictation.applySettings()
            self.overlay.restyle(self.settings.overlay)
            self.overlay.position = self.settings.overlayPosition
            self.updateIcon()
        }
        updateIcon()

        requestMicrophone()
        startTap()
        // Keeps the menu's view of the PC current, as docs/mac-client.md suggests.
        healthTimer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                if case .idle = self?.dictation.state {
                    self?.dictation.checkServer()
                }
            }
        }
        if settings.engine == .server && settings.serverConnection == nil {
            showSettings()
        }
    }

    /// A menu bar app shows no menu bar of its own, but text fields still take Cmd+V, Cmd+C, and the
    /// rest from the main menu's key equivalents. Without an Edit menu, nothing can be pasted into Settings.
    private func installMainMenu() {
        let app = NSMenu()
        app.addItem(withTitle: "Quit Parakeet Dictate", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        let edit = NSMenu(title: "Edit")
        edit.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        edit.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "Z")
        edit.addItem(.separator())
        edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        let window = NSMenu(title: "Window")
        window.addItem(withTitle: "Close", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        let main = NSMenu()
        for submenu in [app, edit, window] {
            let item = NSMenuItem()
            item.submenu = submenu
            main.addItem(item)
        }
        NSApp.mainMenu = main
    }

    // MARK: Permissions

    private func requestMicrophone() {
        if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
            AVCaptureDevice.requestAccess(for: .audio) { granted in
                Log.info("microphone access \(granted ? "granted" : "denied")")
            }
        }
    }

    private var accessibilityTrusted: Bool { AXIsProcessTrusted() }

    /// The event tap needs the Accessibility permission, and so does posting Cmd+V.
    private func startTap() {
        if accessibilityTrusted && dictation.tap.start() {
            Log.info("listening for \(settings.hotkey.title)")
            updateIcon()
            return
        }
        let prompt = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary)
        Log.warning("no Accessibility permission yet; waiting for it")
        updateIcon()
        permissionPoll?.invalidate()
        permissionPoll = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] timer in
            MainActor.assumeIsolated {
                guard let self, self.accessibilityTrusted, self.dictation.tap.start() else {
                    return
                }
                timer.invalidate()
                Log.info("Accessibility granted; listening for \(self.settings.hotkey.title)")
                self.updateIcon()
            }
        }
    }

    // MARK: Status item

    private func updateIcon() {
        let symbol: String
        switch dictation.state {
        case .recording: symbol = "mic.fill"
        case .transcribing, .polishing, .awaitingRelease: symbol = "ellipsis.circle"
        case .idle:
            if !dictation.tap.running || problem != nil {
                symbol = "exclamationmark.triangle"
            } else if case .downloading = dictation.localStatus, settings.engine == .local {
                symbol = "arrow.down.circle"
            } else {
                symbol = "waveform"
            }
        }
        let image = NSImage(systemSymbolName: symbol, accessibilityDescription: "Parakeet Dictate")
        image?.isTemplate = true
        statusItem.button?.image = image
        statusItem.button?.toolTip = "Parakeet Dictate: \(engineLine)"
    }

    /// Something that stops dictation from working, for the menu's first line.
    private var problem: String? {
        if !dictation.tap.running {
            return "Needs the Accessibility permission"
        }
        if AVCaptureDevice.authorizationStatus(for: .audio) == .denied {
            return "Needs the Microphone permission"
        }
        switch settings.engine {
        case .local:
            if case .failed(let reason) = dictation.localStatus {
                return "Local model failed: \(reason)"
            }
        case .server:
            switch dictation.serverStatus {
            case .notConfigured: return "No PC set up in Settings"
            case .failed(.unauthorized): return "The PC rejected the token"
            case .failed(.loading): return nil
            case .failed(let error): return "\(error)"
            default: return nil
            }
        }
        return nil
    }

    private var engineLine: String {
        if let problem {
            return problem
        }
        switch settings.engine {
        case .local:
            switch dictation.localStatus {
            case .notLoaded, .loading: return "Loading the local model…"
            case .downloading(let fraction): return "Downloading the local model… \(Int(fraction * 100))%"
            case .ready: return "Transcribing on this Mac"
            case .failed(let reason): return "Local model failed: \(reason)"
            }
        case .server:
            switch dictation.serverStatus {
            case .connected: return "Transcribing on the PC (\(settings.serverConnection?.baseURL.host ?? ""))"
            case .failed(.loading): return "The PC is still loading its model"
            default: return "Checking the PC…"
            }
        }
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let status = NSMenuItem(title: engineLine, action: nil, keyEquivalent: "")
        status.isEnabled = false
        menu.addItem(status)
        menu.addItem(.separator())

        for engine in Engine.allCases {
            let item = NSMenuItem(title: "Transcribe on \(engine.title.lowercased())", action: #selector(chooseEngine(_:)), keyEquivalent: "")
            item.representedObject = engine.rawValue
            item.state = settings.engine == engine ? .on : .off
            item.target = self
            menu.addItem(item)
        }
        menu.addItem(.separator())

        let polish = NSMenuItem(title: "Post-process with GLM", action: #selector(togglePostprocess), keyEquivalent: "")
        polish.state = settings.postprocess ? .on : .off
        polish.target = self
        if settings.engine == .local && settings.apiKey == nil {
            polish.title = "Post-process with GLM (add a Z.ai key in Settings)"
            polish.action = nil
        } else if settings.engine == .server, case .connected(false) = dictation.serverStatus, settings.postprocess {
            polish.title = "Post-process with GLM (the PC has no GLM key)"
        }
        menu.addItem(polish)
        let overlayItem = NSMenuItem(title: "Overlay", action: nil, keyEquivalent: "")
        let overlayMenu = NSMenu()
        for (title, value) in [("Fluid", OverlayStyle.fluid.rawValue), ("Bars", OverlayStyle.bars.rawValue), ("Off", "off")] {
            let choice = NSMenuItem(title: title, action: #selector(chooseOverlay(_:)), keyEquivalent: "")
            choice.representedObject = value
            choice.state = (settings.overlay?.rawValue ?? "off") == value ? .on : .off
            choice.target = self
            overlayMenu.addItem(choice)
        }
        overlayMenu.addItem(.separator())
        for (title, position) in [("Bottom", OverlayPosition.bottom), ("Top", OverlayPosition.top)] {
            let choice = NSMenuItem(title: title, action: #selector(chooseOverlayPosition(_:)), keyEquivalent: "")
            choice.representedObject = position.rawValue
            choice.state = settings.overlayPosition == position ? .on : .off
            choice.target = self
            overlayMenu.addItem(choice)
        }
        overlayItem.submenu = overlayMenu
        menu.addItem(overlayItem)
        let sounds = NSMenuItem(title: "Sounds", action: #selector(toggleSounds), keyEquivalent: "")
        sounds.state = settings.sounds ? .on : .off
        sounds.target = self
        menu.addItem(sounds)
        let media = NSMenuItem(title: "Pause Media While Recording", action: #selector(togglePauseMedia), keyEquivalent: "")
        media.state = settings.pauseMedia ? .on : .off
        media.target = self
        menu.addItem(media)
        let login = NSMenuItem(title: "Open at Login", action: #selector(toggleLogin), keyEquivalent: "")
        login.state = SMAppService.mainApp.status == .enabled ? .on : .off
        login.target = self
        menu.addItem(login)
        menu.addItem(.separator())

        if !dictation.tap.running {
            menu.addItem(item("Grant Accessibility Access…", #selector(openAccessibility)))
        }
        if AVCaptureDevice.authorizationStatus(for: .audio) == .denied {
            menu.addItem(item("Grant Microphone Access…", #selector(openMicrophone)))
        }
        menu.addItem(item("Settings…", #selector(showSettings), key: ","))
        menu.addItem(item("Open Log", #selector(openLog)))
        menu.addItem(.separator())
        menu.addItem(item("Quit Parakeet Dictate", #selector(quit), key: "q"))
        if settings.engine == .server, case .idle = dictation.state {
            dictation.checkServer()
        }
    }

    private func item(_ title: String, _ action: Selector, key: String = "") -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.target = self
        return item
    }

    @objc private func chooseEngine(_ sender: NSMenuItem) {
        if let raw = sender.representedObject as? String, let engine = Engine(rawValue: raw) {
            settings.engine = engine
            if engine == .server && settings.serverConnection == nil {
                showSettings()
            }
            updateIcon()
        }
    }

    @objc private func togglePostprocess() { settings.postprocess.toggle() }

    @objc private func chooseOverlay(_ sender: NSMenuItem) {
        settings.overlay = (sender.representedObject as? String).flatMap(OverlayStyle.init(rawValue:))
    }

    @objc private func chooseOverlayPosition(_ sender: NSMenuItem) {
        if let position = (sender.representedObject as? String).flatMap(OverlayPosition.init(rawValue:)) {
            settings.overlayPosition = position
        }
    }
    @objc private func toggleSounds() { settings.sounds.toggle() }

    @objc private func togglePauseMedia() { settings.pauseMedia.toggle() }

    @objc private func toggleLogin() {
        do {
            if SMAppService.mainApp.status == .enabled {
                try SMAppService.mainApp.unregister()
            } else {
                try SMAppService.mainApp.register()
            }
        } catch {
            Log.error("open at login: \(error)")
        }
    }

    @objc private func openAccessibility() {
        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")!)
    }

    @objc private func openMicrophone() {
        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")!)
    }

    @objc private func openLog() {
        NSWorkspace.shared.open(Log.fileURL)
    }

    @objc private func quit() {
        NSApp.terminate(nil)
    }

    func applicationWillTerminate(_ notification: Notification) {
        guard let dictation, dictation.media.paused else {
            return
        }
        // MediaRemote sends asynchronously: a process that exits right after the command never sends it.
        dictation.media.recordingStopped()
        RunLoop.current.run(until: Date().addingTimeInterval(1))
    }

    @objc func showSettings() {
        if settingsWindow == nil {
            let view = SettingsView(settings: settings, test: ConnectionTest()) { [weak self] in
                await self?.testConnection() ?? ""
            }
            let window = NSWindow(contentViewController: NSHostingController(rootView: view))
            window.title = "Parakeet Dictate Settings"
            window.styleMask = [.titled, .closable]
            window.isReleasedWhenClosed = false
            window.center()
            settingsWindow = window
        }
        NSApp.activate(ignoringOtherApps: true)
        settingsWindow?.makeKeyAndOrderFront(nil)
    }

    private func testConnection() async -> String {
        guard let connection = settings.serverConnection else {
            return "Enter the PC's URL and token first."
        }
        do {
            let postprocess = try await ServerClient(connection: connection).health()
            dictation.checkServer()
            return postprocess ? "Connected. The PC post-processes with GLM." : "Connected. The PC has GLM off or no key."
        } catch let error as ServerError {
            switch error {
            case .unauthorized: return "The PC rejected the token."
            case .unreachable(let reason):
                return "No answer (\(reason)). Check that Server for Mac is on and the firewall rule is in place."
            default: return "\(error)"
            }
        } catch {
            return "\(error)"
        }
    }
}
