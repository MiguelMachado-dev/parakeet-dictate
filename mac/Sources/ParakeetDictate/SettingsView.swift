import AVFoundation
import AppKit
import DictateCore
import SwiftUI

/// What the Settings window shows about the permissions and the engine. AppDelegate keeps it current.
final class AppStatus: ObservableObject {
    @Published var microphone: AVAuthorizationStatus = .notDetermined
    @Published var accessibility = false
    @Published var local: LocalEngine.Status = .notLoaded
    @Published var server: ServerStatus = .notConfigured
    @Published var openAtLogin = false
}

struct TestResult: Equatable {
    let ok: Bool
    let message: String
}

/// A test button's transient state. A class rather than @State: SwiftUI's @State is a macro whose plugin
/// ships with Xcode, and this package builds with the Command Line Tools alone.
final class TestRun: ObservableObject {
    @Published var running = false
    @Published var result: TestResult?
}

/// Whether Advanced is open. A DisclosureGroup in the grouped form never opened, by click or by VoiceOver.
final class Disclosure: ObservableObject {
    @Published var expanded = false
}

/// What the Settings window asks of the app.
struct SettingsActions {
    let requestMicrophone: () -> Void
    let openMicrophoneSettings: () -> Void
    let openAccessibilitySettings: () -> Void
    let setOpenAtLogin: (Bool) -> Void
    let testConnection: () async -> TestResult
}

/// One line of status: a colored symbol, or a spinner while something is under way.
struct StatusLine: View {
    enum Level {
        case ok, working, problem
    }

    let level: Level
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            switch level {
            case .ok:
                Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
            case .working:
                ProgressView().controlSize(.small)
            case .problem:
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            }
            Text(text).textSelection(.enabled)
        }
    }
}

extension TestResult {
    var line: StatusLine { StatusLine(level: ok ? .ok : .problem, text: message) }
}

private func note(_ text: String) -> some View {
    Text(text).font(.caption).foregroundStyle(.secondary)
}

/// Each tab is a fixed-width form as tall as its content; the window follows the selected tab's height.
private extension View {
    func settingsPage() -> some View {
        formStyle(.grouped)
            .frame(width: 520)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// The engine's state for the status rows: the local model's progress, or the PC's last health check.
func engineStatus(_ engine: Engine, _ status: AppStatus) -> StatusLine {
    switch engine {
    case .local:
        switch status.local {
        case .notLoaded, .loading: return StatusLine(level: .working, text: "Loading the model…")
        case .downloading(let fraction):
            return StatusLine(level: .working, text: "Downloading the model… \(Int(fraction * 100))%")
        case .ready: return StatusLine(level: .ok, text: "Ready on this Mac")
        case .failed(let reason): return StatusLine(level: .problem, text: "The model failed: \(reason)")
        }
    case .server:
        switch status.server {
        case .notConfigured: return StatusLine(level: .problem, text: "Enter the PC's URL and token")
        case .unknown: return StatusLine(level: .working, text: "Checking the PC…")
        case .connected(let postprocess):
            return StatusLine(level: .ok, text: postprocess ? "Connected to the PC" : "Connected; the PC has post-processing off or no key")
        case .failed(.loading): return StatusLine(level: .working, text: "The PC is still loading its model")
        case .failed(.unauthorized): return StatusLine(level: .problem, text: "The PC rejected the token")
        case .failed(let error): return StatusLine(level: .problem, text: "\(error)")
        }
    }
}

// MARK: General

struct GeneralSettingsView: View {
    @ObservedObject var settings: Settings
    @ObservedObject var status: AppStatus
    let actions: SettingsActions

    var body: some View {
        Form {
            Section {
                LabeledContent("Microphone") {
                    HStack {
                        switch status.microphone {
                        case .authorized:
                            StatusLine(level: .ok, text: "Allowed")
                        case .notDetermined:
                            StatusLine(level: .problem, text: "Not asked yet")
                            Button("Allow…", action: actions.requestMicrophone)
                        default:
                            StatusLine(level: .problem, text: "Not allowed")
                            Button("Open System Settings", action: actions.openMicrophoneSettings)
                        }
                    }
                }
                LabeledContent("Accessibility") {
                    HStack {
                        if status.accessibility {
                            StatusLine(level: .ok, text: "Allowed")
                        } else {
                            StatusLine(level: .problem, text: "Not allowed")
                            Button("Open System Settings", action: actions.openAccessibilitySettings)
                        }
                    }
                }
                LabeledContent("Transcription") {
                    engineStatus(settings.engine, status)
                }
            } header: {
                Text("Status")
            } footer: {
                note("Accessibility lets the app hear the hotkey and paste with Cmd+V.")
            }

            Section {
                Picker("Hotkey", selection: $settings.hotkey) {
                    ForEach(Hotkey.allCases) { Text($0.title).tag($0) }
                }
            } footer: {
                note("Hold the hotkey to talk, or tap it for hands-free until the next press. Esc cancels.")
            }

            Section("Overlay") {
                Picker("Style", selection: $settings.overlay) {
                    Text("Fluid").tag(OverlayStyle?.some(.fluid))
                    Text("Bars").tag(OverlayStyle?.some(.bars))
                    Text("Off").tag(OverlayStyle?.none)
                }
                Picker("Position", selection: $settings.overlayPosition) {
                    Text("Bottom").tag(OverlayPosition.bottom)
                    Text("Top").tag(OverlayPosition.top)
                }
                .disabled(settings.overlay == nil)
            }

            Section {
                Toggle("Add a space after each dictation", isOn: $settings.appendSpace)
                Toggle("Sounds", isOn: $settings.sounds)
                Toggle("Pause media while recording", isOn: $settings.pauseMedia)
                Toggle("Open at login", isOn: Binding(get: { status.openAtLogin }, set: actions.setOpenAtLogin))
            }
        }
        .settingsPage()
    }
}

// MARK: Transcription

struct TranscriptionSettingsView: View {
    @ObservedObject var settings: Settings
    @ObservedObject var status: AppStatus
    @ObservedObject var test: TestRun
    let actions: SettingsActions

    var body: some View {
        Form {
            Section {
                Picker("Transcribe on", selection: $settings.engine) {
                    ForEach(Engine.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                LabeledContent("Status") {
                    engineStatus(settings.engine, status)
                }
                if settings.engine == .local, case .downloading(let fraction) = status.local {
                    ProgressView(value: fraction)
                }
            } footer: {
                note(
                    settings.engine == .local
                        ? "Parakeet Ultra runs on this Mac's Neural Engine. The first start downloads about 615 MB."
                        : "The PC transcribes and post-processes. Turn on Server for Mac in the Windows tray."
                )
            }

            if settings.engine == .server {
                Section {
                    TextField("URL", text: $settings.serverURL, prompt: Text("http://192.168.3.2:8765"))
                    SecureField("Token", text: $settings.serverToken)
                    HStack {
                        Button("Paste Connection Info") { pasteConnectionInfo() }
                            .help("Paste what the Windows tray's \"Copy Mac connection info\" copied: the URL and the token.")
                        Button("Test Connection") {
                            test.running = true
                            test.result = nil
                            Task { @MainActor in
                                test.result = await actions.testConnection()
                                test.running = false
                            }
                        }
                        .disabled(test.running)
                        if test.running {
                            ProgressView().controlSize(.small)
                        }
                    }
                    if let result = test.result {
                        result.line.font(.callout)
                    }
                } header: {
                    Text("The PC")
                } footer: {
                    note("Copy Mac connection info, in the Windows tray, copies the URL and the token on two lines.")
                }
            }
        }
        .settingsPage()
    }

    private func pasteConnectionInfo() {
        guard let text = NSPasteboard.general.string(forType: .string), let info = ServerConnection.parse(text) else {
            test.result = TestResult(ok: false, message: "The clipboard does not hold a URL and a token on two lines.")
            return
        }
        settings.serverURL = info.baseURL.absoluteString
        settings.serverToken = info.token
        test.result = TestResult(ok: true, message: "Pasted. Test the connection to check it.")
    }
}

// MARK: Post-processing

struct PostprocessingSettingsView: View {
    @ObservedObject var settings: Settings
    @ObservedObject var test: TestRun
    @ObservedObject var advancedShown: Disclosure

    var body: some View {
        Form {
            Section {
                Toggle("Post-process", isOn: $settings.postprocess)
            } footer: {
                note(
                    settings.engine == .local
                        ? "Removes fillers and fixes punctuation before pasting. The transcript goes to the provider below; "
                            + "the audio stays on this Mac. App rules post-process even with this off."
                        : "The PC post-processes with its own provider, key, and extra instructions; change them in the "
                            + "Windows app's Settings. This switch is sent with each dictation."
                )
            }
            // On the PC, the PC's own provider, key, and instructions apply: the rest would do nothing.
            if settings.engine == .local {
                provider
                if settings.polishProvider == .anthropic {
                    claude
                }
                Section {
                    TextField(
                        "Extra instructions", text: $settings.extraInstructions,
                        prompt: Text("Names: Miguel, João. Terms: Parakeet, Z.ai, GLM."), axis: .vertical
                    )
                    .lineLimit(2...4)
                } footer: {
                    note("Added to the prompt, for names and terms to spell your way.")
                }
                Section {
                    Button {
                        advancedShown.expanded.toggle()
                    } label: {
                        HStack {
                            Text("Advanced")
                            Spacer()
                            Image(systemName: advancedShown.expanded ? "chevron.up" : "chevron.down")
                                .foregroundStyle(.secondary)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    if advancedShown.expanded {
                        advanced
                    }
                }
            }
        }
        .settingsPage()
        .onChange(of: settings.polishProvider) { test.result = nil }
    }

    private var provider: some View {
        Section {
            Picker("Provider", selection: $settings.polishProvider) {
                ForEach(PolishProvider.allCases) { Text($0.title).tag($0) }
            }
            switch settings.polishProvider {
            case .zai: SecureField("Z.ai API key", text: $settings.zaiAPIKey)
            case .anthropic: SecureField("Anthropic API key", text: $settings.anthropicAPIKey)
            }
            if settings.apiKey == nil {
                StatusLine(level: .problem, text: "No key: dictations paste the raw transcript.").font(.callout)
            } else {
                HStack {
                    Button("Test \(settings.polishProvider.modelName)") { testProvider() }
                        .disabled(test.running)
                    if test.running {
                        ProgressView().controlSize(.small)
                    }
                }
                if let result = test.result {
                    result.line.font(.callout)
                }
            }
        }
    }

    private var claude: some View {
        Section {
            Picker("Model", selection: $settings.claudeModel) {
                ForEach(ClaudeModel.allCases) { Text($0.title).tag($0) }
            }
            if settings.claudeModel == .other {
                TextField("Model ID", text: $settings.claudeOtherModel, prompt: Text(ClaudeConfig.haiku))
            }
            Toggle("Thinking", isOn: $settings.claudeThinking)
            Picker("Effort", selection: $settings.claudeEffort) {
                ForEach(ClaudeEffort.allCases) { Text($0.title).tag($0) }
            }
        } header: {
            Text("Claude")
        } footer: {
            note("Thinking off is the fastest. The API takes Extra high and Max only with thinking, so choosing either turns it on.")
        }
    }

    @ViewBuilder private var advanced: some View {
        switch settings.polishProvider {
        case .zai:
            TextField("Model", text: $settings.glmModel, prompt: Text(GlmConfig().model))
            TextField("Endpoint", text: $settings.glmEndpoint, prompt: Text(GlmConfig().endpoint.absoluteString))
            if !settings.glmEndpoint.trimmingCharacters(in: .whitespaces).isEmpty && settings.glmEndpointURL == nil {
                StatusLine(level: .problem, text: "Not an http(s) URL; the default endpoint is used.").font(.callout)
            }
            Picker("Reasoning effort", selection: $settings.glmReasoningEffort) {
                ForEach(GlmEffort.allCases) { Text($0.title).tag($0) }
            }
            timeout($settings.glmTimeout)
        case .anthropic:
            timeout($settings.claudeTimeout)
        }
    }

    private func timeout(_ value: Binding<TimeInterval>) -> some View {
        LabeledContent("Timeout") {
            HStack {
                Text("\(String(format: "%g", value.wrappedValue)) s").monospacedDigit()
                Stepper("Timeout", value: value, in: 2...30, step: 1).labelsHidden()
            }
        }
        .help("How long to wait for a reply before pasting the raw transcript.")
    }

    private func testProvider() {
        guard let polisher = settings.makePolisher() else {
            return
        }
        let name = settings.polishProvider.modelName
        test.running = true
        test.result = nil
        Task { @MainActor in
            let started = Date()
            do {
                let text = try await polisher.polish("um so this is uh a dictation test", instructions: "")
                let seconds = String(format: "%.1f", Date().timeIntervalSince(started))
                test.result = TestResult(ok: true, message: "\(name) replied in \(seconds) s: \(text)")
            } catch let error as PolishError {
                test.result = TestResult(ok: false, message: "\(name): \(error)")
            } catch {
                test.result = TestResult(ok: false, message: "\(name): \(error.localizedDescription)")
            }
            test.running = false
        }
    }
}

// MARK: Window

enum SettingsTab: String, CaseIterable {
    case general, transcription, postprocessing, dictionary, rules

    var title: String {
        switch self {
        case .general: return "General"
        case .transcription: return "Transcription"
        case .postprocessing: return "Post-processing"
        case .dictionary: return "Dictionary"
        case .rules: return "App Rules"
        }
    }

    var symbol: String {
        switch self {
        case .general: return "gearshape"
        case .transcription: return "waveform"
        case .postprocessing: return "wand.and.stars"
        case .dictionary: return "character.book.closed"
        case .rules: return "list.bullet.rectangle"
        }
    }
}

/// The Settings window's toolbar tabs. The window takes each tab's size, and reopens on the last tab.
final class SettingsTabs: NSTabViewController {
    private static let lastTabKey = "settingsTab"

    init(pages: [(SettingsTab, NSViewController)]) {
        super.init(nibName: nil, bundle: nil)
        tabStyle = .toolbar
        for (tab, controller) in pages {
            controller.title = tab.title
            let item = NSTabViewItem(viewController: controller)
            item.identifier = tab.rawValue
            item.label = tab.title
            item.image = NSImage(systemSymbolName: tab.symbol, accessibilityDescription: tab.title)
            addTabViewItem(item)
        }
        let last = UserDefaults.standard.string(forKey: Self.lastTabKey).flatMap(SettingsTab.init(rawValue:))
        select(last ?? .general)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    func select(_ tab: SettingsTab) {
        if let index = tabViewItems.firstIndex(where: { $0.identifier as? String == tab.rawValue }) {
            selectedTabViewItemIndex = index
        }
    }

    override func tabView(_ tabView: NSTabView, didSelect item: NSTabViewItem?) {
        super.tabView(tabView, didSelect: item)
        if let id = item?.identifier as? String {
            UserDefaults.standard.set(id, forKey: Self.lastTabKey)
        }
        fitWindow()
    }

    override func preferredContentSizeDidChange(for viewController: NSViewController) {
        super.preferredContentSizeDidChange(for: viewController)
        if viewController === tabViewItems[safe: selectedTabViewItemIndex]?.viewController {
            fitWindow()
        }
    }

    /// Resizes the window to the selected tab, keeping its top edge where it is.
    func fitWindow() {
        guard let window = view.window, let controller = tabViewItems[safe: selectedTabViewItemIndex]?.viewController else {
            return
        }
        let size = controller.preferredContentSize
        guard size.width > 0, size.height > 0, window.contentLayoutRect.size != size else {
            return
        }
        let top = window.frame.maxY
        window.setContentSize(size)
        window.setFrameTopLeftPoint(NSPoint(x: window.frame.minX, y: top))
    }
}

private extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
