import DictateCore
import Foundation
import Security

enum Engine: String, CaseIterable, Identifiable {
    /// Parakeet on this Mac's Neural Engine, then GLM or Claude straight from the Mac.
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

/// Who post-processes on this Mac. The PC uses its own provider, whatever is chosen here.
enum PolishProvider: String, CaseIterable, Identifiable {
    case zai
    case anthropic

    var id: String { rawValue }

    var title: String {
        switch self {
        case .zai: return "Z.ai (GLM)"
        case .anthropic: return "Anthropic (Claude)"
        }
    }

    var modelName: String {
        switch self {
        case .zai: return "GLM"
        case .anthropic: return "Claude"
        }
    }

    var keyName: String {
        switch self {
        case .zai: return "Z.ai"
        case .anthropic: return "Anthropic"
        }
    }
}

enum ClaudeModel: String, CaseIterable, Identifiable {
    case haiku = "claude-haiku-5-5"
    case sonnet = "claude-sonnet-5-5"
    /// A model ID typed in Settings.
    case other

    var id: String { rawValue }

    var title: String {
        switch self {
        case .haiku: return "Claude Haiku 5.5"
        case .sonnet: return "Claude Sonnet 5.5"
        case .other: return "Other"
        }
    }
}

enum ClaudeEffort: String, CaseIterable, Identifiable {
    case low, medium, high, xhigh, max

    var id: String { rawValue }

    var title: String {
        switch self {
        case .low: return "Low"
        case .medium: return "Medium"
        case .high: return "High"
        case .xhigh: return "Extra high"
        case .max: return "Max"
        }
    }

    /// The API rejects thinking off above high effort.
    var allowsThinkingOff: Bool { self != .xhigh && self != .max }
}

/// GLM-5.3 always reasons; higher effort is slower.
enum GlmEffort: String, CaseIterable, Identifiable {
    case low, high, max

    var id: String { rawValue }

    var title: String {
        switch self {
        case .low: return "Low"
        case .high: return "High"
        case .max: return "Max"
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

/// Choices live in UserDefaults; the PC token and the API keys live in the Keychain.
final class Settings: ObservableObject {
    private let defaults = UserDefaults.standard
    /// Called after any change, on the main thread. Text fields call it once typing pauses, or on `flush()`.
    var onChange: (() -> Void)?
    private var pendingWrites: [String: (Settings) -> Void] = [:]
    private var pendingFlush: DispatchWorkItem?

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
        didSet { saveLater("serverURL") { $0.defaults.set($0.serverURL, forKey: "serverURL") } }
    }
    @Published var polishProvider: PolishProvider {
        didSet { save(polishProvider.rawValue, "polishProvider") }
    }
    @Published var claudeModel: ClaudeModel {
        didSet { save(claudeModel.rawValue, "claudeModel") }
    }
    /// The model ID when `claudeModel` is `.other`.
    @Published var claudeOtherModel: String {
        didSet { saveLater("claudeOtherModel") { $0.defaults.set($0.claudeOtherModel, forKey: "claudeOtherModel") } }
    }
    @Published var claudeEffort: ClaudeEffort {
        didSet {
            // The API takes efforts above high only with thinking on, so choosing one turns it on.
            if !claudeEffort.allowsThinkingOff && !claudeThinking {
                claudeThinking = true
            }
            save(claudeEffort.rawValue, "claudeEffort")
        }
    }
    @Published var claudeThinking: Bool {
        didSet {
            if !claudeThinking && !claudeEffort.allowsThinkingOff {
                claudeEffort = .high
            }
            save(claudeThinking, "claudeThinking")
        }
    }
    /// The advanced settings: "" or 0 keeps GlmConfig's and ClaudeConfig's defaults.
    @Published var glmModel: String {
        didSet { saveLater("glmModel") { $0.defaults.set($0.glmModel, forKey: "glmModel") } }
    }
    @Published var glmEndpoint: String {
        didSet { saveLater("glmEndpoint") { $0.defaults.set($0.glmEndpoint, forKey: "glmEndpoint") } }
    }
    @Published var glmReasoningEffort: GlmEffort {
        didSet { save(glmReasoningEffort.rawValue, "glmReasoningEffort") }
    }
    @Published var glmTimeout: TimeInterval {
        didSet { save(glmTimeout, "glmTimeout") }
    }
    @Published var claudeTimeout: TimeInterval {
        didSet { save(claudeTimeout, "claudeTimeout") }
    }
    @Published var extraInstructions: String {
        didSet { saveLater("extraInstructions") { $0.defaults.set($0.extraInstructions, forKey: "extraInstructions") } }
    }
    /// Saved without calling onChange: nothing needs rebuilding, since each dictation reads them as it ends.
    @Published var rules: [AppRule] {
        didSet { defaults.set(try? JSONEncoder().encode(rules), forKey: "appRules") }
    }
    /// Read at recording boundaries; editing the dictionary does not reload the audio engine.
    @Published var dictionary: PersonalDictionary {
        didSet { defaults.set(try? JSONEncoder().encode(dictionary), forKey: "personalDictionary") }
    }
    @Published var serverToken: String {
        didSet { saveLater("serverToken") { Keychain.set($0.serverToken, for: "server-token") } }
    }
    @Published var zaiAPIKey: String {
        didSet { saveLater("zaiAPIKey") { Keychain.set($0.zaiAPIKey, for: "zai-api-key") } }
    }
    @Published var anthropicAPIKey: String {
        didSet { saveLater("anthropicAPIKey") { Keychain.set($0.anthropicAPIKey, for: "anthropic-api-key") } }
    }

    init() {
        defaults.register(defaults: [
            "engine": Engine.local.rawValue, "hotkey": Hotkey.rightCommand.rawValue, "postprocess": true,
            "appendSpace": true, "sounds": true, "pauseMedia": true, "serverURL": "", "extraInstructions": "",
            "overlay": OverlayStyle.fluid.rawValue, "overlayPosition": OverlayPosition.bottom.rawValue,
            "polishProvider": PolishProvider.zai.rawValue, "claudeModel": ClaudeModel.haiku.rawValue,
            "claudeOtherModel": "", "claudeEffort": ClaudeEffort.low.rawValue, "claudeThinking": false,
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
        polishProvider = PolishProvider(rawValue: defaults.string(forKey: "polishProvider") ?? "") ?? .zai
        claudeModel = ClaudeModel(rawValue: defaults.string(forKey: "claudeModel") ?? "") ?? .haiku
        claudeOtherModel = defaults.string(forKey: "claudeOtherModel") ?? ""
        let thinking = defaults.bool(forKey: "claudeThinking")
        let effort = ClaudeEffort(rawValue: defaults.string(forKey: "claudeEffort") ?? "") ?? .low
        claudeThinking = thinking
        // didSet does not run here, so a `defaults write` past high with thinking off is clamped like the toggle does.
        claudeEffort = thinking || effort.allowsThinkingOff ? effort : .high
        extraInstructions = defaults.string(forKey: "extraInstructions") ?? ""
        glmModel = defaults.string(forKey: "glmModel") ?? ""
        glmEndpoint = defaults.string(forKey: "glmEndpoint") ?? ""
        glmReasoningEffort = GlmEffort(rawValue: defaults.string(forKey: "glmReasoningEffort") ?? "") ?? .low
        let glmTimeout = defaults.double(forKey: "glmTimeout")
        self.glmTimeout = glmTimeout > 0 ? glmTimeout : GlmConfig().timeout
        let claudeTimeout = defaults.double(forKey: "claudeTimeout")
        self.claudeTimeout = claudeTimeout > 0 ? claudeTimeout : ClaudeConfig().timeout
        rules = defaults.data(forKey: "appRules").flatMap { try? JSONDecoder().decode([AppRule].self, from: $0) } ?? []
        dictionary = defaults.data(forKey: "personalDictionary")
            .flatMap { try? JSONDecoder().decode(PersonalDictionary.self, from: $0) } ?? PersonalDictionary()
        serverToken = Keychain.get("server-token") ?? ""
        zaiAPIKey = Keychain.get("zai-api-key") ?? ""
        anthropicAPIKey = Keychain.get("anthropic-api-key") ?? ""
    }

    var serverConnection: ServerConnection? {
        let token = serverToken.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty, let url = ServerConnection.normalizedURL(serverURL) else {
            return nil
        }
        return ServerConnection(baseURL: url, token: token)
    }

    /// The GLM settings of config.example.toml.
    var glmConfig: GlmConfig {
        var config = GlmConfig()
        config.extraInstructions = extraInstructions
        config.reasoningEffort = glmReasoningEffort.rawValue
        let model = glmModel.trimmingCharacters(in: .whitespacesAndNewlines)
        if !model.isEmpty {
            config.model = model
        }
        if let endpoint = glmEndpointURL {
            config.endpoint = endpoint
        }
        if glmTimeout > 0 {
            config.timeout = glmTimeout
        }
        return config
    }

    /// The typed endpoint, or nil when it is empty or not an http(s) URL, which keeps the default.
    var glmEndpointURL: URL? {
        let text = glmEndpoint.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: text), ["http", "https"].contains(url.scheme?.lowercased()), url.host != nil else {
            return nil
        }
        return url
    }

    var claudeConfig: ClaudeConfig {
        var config = ClaudeConfig()
        config.extraInstructions = extraInstructions
        config.effort = claudeEffort.rawValue
        config.thinking = claudeThinking
        let otherModel = claudeOtherModel.trimmingCharacters(in: .whitespacesAndNewlines)
        if claudeModel != .other {
            config.model = claudeModel.rawValue
        } else if !otherModel.isEmpty {
            config.model = otherModel
        }
        if claudeTimeout > 0 {
            config.timeout = claudeTimeout
        }
        return config
    }

    /// The chosen provider's key, or nil when it has none.
    var apiKey: String? {
        let key = (polishProvider == .zai ? zaiAPIKey : anthropicAPIKey).trimmingCharacters(in: .whitespacesAndNewlines)
        return key.isEmpty ? nil : key
    }

    /// A client for the chosen provider, or nil without its key.
    func makePolisher() -> (any Polisher)? {
        guard let key = apiKey else {
            return nil
        }
        switch polishProvider {
        case .zai: return GlmClient(config: glmConfig, apiKey: key)
        case .anthropic: return ClaudeClient(config: claudeConfig, apiKey: key)
        }
    }

    /// Seconds to wait for the chosen provider before pasting the raw transcript.
    var polishTimeout: TimeInterval {
        polishProvider == .zai ? glmConfig.timeout : claudeConfig.timeout
    }

    private func save(_ value: Any, _ key: String) {
        defaults.set(value, forKey: key)
        onChange?()
    }

    /// Text fields change on every keystroke. Their writes wait until typing pauses, so a keystroke
    /// neither touches the Keychain nor rebuilds the clients and pings the PC.
    private func saveLater(_ key: String, _ write: @escaping (Settings) -> Void) {
        pendingWrites[key] = write
        pendingFlush?.cancel()
        let flush = DispatchWorkItem { [weak self] in self?.flush() }
        pendingFlush = flush
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5, execute: flush)
    }

    /// Saves the text still waiting on a pause in typing, then applies it.
    func flush() {
        pendingFlush?.cancel()
        pendingFlush = nil
        guard !pendingWrites.isEmpty else {
            return
        }
        let writes = pendingWrites.values
        pendingWrites.removeAll()
        writes.forEach { $0(self) }
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
