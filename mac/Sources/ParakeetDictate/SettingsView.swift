import AppKit
import DictateCore
import SwiftUI

/// The window's transient state. A class rather than @State: SwiftUI's @State is a macro whose plugin
/// ships with Xcode, and this package builds with the Command Line Tools alone.
final class ConnectionTest: ObservableObject {
    @Published var result = ""
    @Published var running = false
}

struct SettingsView: View {
    @ObservedObject var settings: Settings
    @ObservedObject var test: ConnectionTest
    let testConnection: () async -> String

    var body: some View {
        Form {
            Section {
                Picker("Transcribe on", selection: $settings.engine) {
                    ForEach(Engine.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                Picker("Hotkey", selection: $settings.hotkey) {
                    ForEach(Hotkey.allCases) { Text($0.title).tag($0) }
                }
                Picker("Overlay", selection: $settings.overlay) {
                    Text("Fluid").tag(OverlayStyle?.some(.fluid))
                    Text("Bars").tag(OverlayStyle?.some(.bars))
                    Text("Off").tag(OverlayStyle?.none)
                }
                Picker("Overlay position", selection: $settings.overlayPosition) {
                    Text("Bottom").tag(OverlayPosition.bottom)
                    Text("Top").tag(OverlayPosition.top)
                }
                Toggle("Add a space after each dictation", isOn: $settings.appendSpace)
                Toggle("Sounds", isOn: $settings.sounds)
                Toggle("Pause media while recording", isOn: $settings.pauseMedia)
            } footer: {
                Text("Hold the hotkey to talk, or tap it for hands-free until the next press. Esc cancels.")
                    .font(.caption).foregroundStyle(.secondary)
            }

            Section("The PC") {
                TextField("URL", text: $settings.serverURL, prompt: Text("http://192.168.3.2:8765"))
                SecureField("Token", text: $settings.serverToken)
                HStack {
                    Button("Paste Connection Info") { pasteConnectionInfo() }
                        .help("Paste what the Windows tray's \"Copy Mac connection info\" copied: the URL and the token.")
                    Button("Test Connection") {
                        test.running = true
                        test.result = "Testing…"
                        Task { @MainActor in
                            test.result = await testConnection()
                            test.running = false
                        }
                    }
                    .disabled(test.running)
                }
                if !test.result.isEmpty {
                    Text(test.result).font(.caption).foregroundStyle(.secondary)
                }
            }

            Section {
                Toggle("Post-process with GLM", isOn: $settings.postprocess)
                SecureField("Z.ai API key", text: $settings.zaiAPIKey)
                TextField(
                    "Extra instructions", text: $settings.extraInstructions,
                    prompt: Text("Names: Miguel, João. Terms: Parakeet, Z.ai, GLM."), axis: .vertical
                )
                .lineLimit(2...4)
            } header: {
                Text("Post-processing")
            } footer: {
                Text(
                    "GLM removes fillers and fixes punctuation. On this Mac it uses the key above; "
                        + "on the PC, the PC's own key and settings."
                )
                .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .frame(width: 500)
        .fixedSize(horizontal: false, vertical: true)
    }

    private func pasteConnectionInfo() {
        guard let text = NSPasteboard.general.string(forType: .string), let info = ServerConnection.parse(text) else {
            test.result = "The clipboard does not hold a URL and a token on two lines."
            return
        }
        settings.serverURL = info.baseURL.absoluteString
        settings.serverToken = info.token
        test.result = "Pasted."
    }
}
