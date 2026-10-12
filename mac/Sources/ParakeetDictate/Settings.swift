import DictateCore
import Foundation
import Security

enum Engine: String, CaseIterable, Identifiable {
    /// Parakeet on this Mac's Neural Engine, then GLM straight from the Mac.
    case local
    /// The Windows PC's server, which transcribes and post-processes.
    case server

    var id: String { rawValue }

    var title: String {
        switch self {
        case .local: return "This Mac"
        case .server: return "The PC"
        }
    }
}

/// The hotkey. Modifier keys arrive as flags-changed events and are told apart from their left twin
/// by their device-dependent flag bit (NX_DEVICE*KEYMASK in IOKit).
enum Hotkey: Int, CaseIterable, Identifiable {
    case rightCommand = 54
    case rightOption = 61
    case rightControl = 62
    case rightShift = 60
    case f13 = 105

    var id: Int { rawValue }
    var keyCode: Int64 { Int64(rawValue) }

    var title: String {
        switch self {
        case .rightCommand: return "Right Command"
        case .rightOption: return "Right Option"
        case .rightControl: return "Right Control"
        case .rightShift: return "Right Shift"
        case .f13: return "F13"
        }
    }

    var modifierMask: UInt64? {
        switch self {
        case .rightCommand: return 0x10
        case .rightOption: return 0x40
        case .rightControl: return 0x2000
        case .rightShift: return 0x04
        case .f13: return nil
        }
    }
}

/// Choices live in UserDefaults; the PC token and the Z.ai key live in the Keychain.
final class Settings: ObservableObject {
    private let defaults = UserDefaults.standard
    /// Called after any change, on the main thread.
    var onChange: (() -> Void)?

    @Published var engine: Engine {
        didSet { save(engine.rawValue, "engine") }
    }
    @Published var hotkey: Hotkey {
        didSet { save(hotkey.rawValue, "hotkey") }
    }
    @Published var postprocess: Bool {
        didSet { save(postprocess, "postprocess") }
    }
    @Published var appendSpace: Bool {
        didSet { save(appendSpace, "appendSpace") }
    }
    @Published var sounds: Bool {
        didSet { save(sounds, "sounds") }
    }
    /// Pauses what macOS shows as Now Playing while recording.
    @Published var pauseMedia: Bool {
        didSet { save(pauseMedia, "pauseMedia") }
    }
    /// Nil turns the overlay off.
    @Published var overlay: OverlayStyle? {
        didSet { save(overlay?.rawValue ?? "off", "overlay") }
    }
    @Published var overlayPosition: OverlayPosition {
        didSet { save(overlayPosition.rawValue, "overlayPosition") }
    }
    @Published var serverURL: String {
        didSet { save(serverURL, "serverURL") }
    }
    @Published var extraInstructions: String {
        didSet { save(extraInstructions, "extraInstructions") }
    }
    @Published var serverToken: String {
        didSet { Keychain.set(serverToken, for: "server-token"); onChange?() }
    }
    @Published var zaiAPIKey: String {
        didSet { Keychain.set(zaiAPIKey, for: "zai-api-key"); onChange?() }
    }

    init() {
        defaults.register(defaults: [
            "engine": Engine.local.rawValue, "hotkey": Hotkey.rightCommand.rawValue, "postprocess": true,
            "appendSpace": true, "sounds": true, "pauseMedia": true, "serverURL": "", "extraInstructions": "",
            "overlay": OverlayStyle.fluid.rawValue, "overlayPosition": OverlayPosition.bottom.rawValue,
        ])
        engine = Engine(rawValue: defaults.string(forKey: "engine") ?? "") ?? .local
        hotkey = Hotkey(rawValue: defaults.integer(forKey: "hotkey")) ?? .rightCommand
        postprocess = defaults.bool(forKey: "postprocess")
        appendSpace = defaults.bool(forKey: "appendSpace")
        sounds = defaults.bool(forKey: "sounds")
        pauseMedia = defaults.bool(forKey: "pauseMedia")
        overlay = OverlayStyle(rawValue: defaults.string(forKey: "overlay") ?? "")
        overlayPosition = OverlayPosition(rawValue: defaults.string(forKey: "overlayPosition") ?? "") ?? .bottom
        serverURL = defaults.string(forKey: "serverURL") ?? ""
        extraInstructions = defaults.string(forKey: "extraInstructions") ?? ""
        serverToken = Keychain.get("server-token") ?? ""
        zaiAPIKey = Keychain.get("zai-api-key") ?? ""
    }

    var serverConnection: ServerConnection? {
        let token = serverToken.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty, let url = ServerConnection.normalizedURL(serverURL) else {
            return nil
        }
        return ServerConnection(baseURL: url, token: token)
    }

    /// The GLM settings of config.example.toml. Only the extra instructions have a field in Settings;
    /// the rest can be changed with `defaults write dev.miguelmachado.parakeet-dictate glmModel glm-5.3`.
    var glmConfig: GlmConfig {
        var config = GlmConfig()
        config.extraInstructions = extraInstructions
        if let model = defaults.string(forKey: "glmModel"), !model.isEmpty {
            config.model = model
        }
        if let endpoint = defaults.string(forKey: "glmEndpoint").flatMap(URL.init(string:)) {
            config.endpoint = endpoint
        }
        if let effort = defaults.string(forKey: "glmReasoningEffort"), !effort.isEmpty {
            config.reasoningEffort = effort
        }
        if defaults.double(forKey: "glmTimeout") > 0 {
            config.timeout = defaults.double(forKey: "glmTimeout")
        }
        return config
    }

    var apiKey: String? {
        let key = zaiAPIKey.trimmingCharacters(in: .whitespacesAndNewlines)
        return key.isEmpty ? nil : key
    }

    private func save(_ value: Any, _ key: String) {
        defaults.set(value, forKey: key)
        onChange?()
    }
}

enum Keychain {
    private static let service = "dev.miguelmachado.parakeet-dictate"

    static func get(_ account: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
            kSecAttrAccount as String: account, kSecReturnData as String: true,
        ]
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess, let data = item as? Data else {
            return nil
        }
        return String(data: data, encoding: .utf8)
    }

    static func set(_ value: String, for account: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        guard !value.isEmpty else {
            SecItemDelete(query as CFDictionary)
            return
        }
        let data = Data(value.utf8)
        var status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var add = query
            add[kSecValueData as String] = data
            status = SecItemAdd(add as CFDictionary, nil)
        }
        if status != errSecSuccess {
            Log.error("Keychain write for \(account) failed: \(status)")
        }
    }
}
