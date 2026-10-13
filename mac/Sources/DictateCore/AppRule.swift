import Foundation

/// Instructions for the post-processing model, such as "Translate to English.", for dictations that go to
/// certain apps.
public struct AppRule: Codable, Equatable, Identifiable, Sendable {
    public struct App: Codable, Equatable, Hashable, Sendable {
        public var bundleID: String
        /// Kept for Settings, so a rule still reads well when the app is gone.
        public var name: String

        public init(bundleID: String, name: String) {
            self.bundleID = bundleID
            self.name = name
        }
    }

    public var id = UUID()
    public var name: String
    public var instructions: String
    public var apps: [App]
    public var enabled = true

    public init(name: String, instructions: String, apps: [App] = []) {
        self.name = name
        self.instructions = instructions
        self.apps = apps
    }

    /// Starting points that Settings offers for a new rule.
    public static let presets = [
        AppRule(name: "Translate to English", instructions: "Translate the transcript to English."),
        AppRule(name: "Casual", instructions: "Make the tone casual and friendly, as in a chat message."),
        AppRule(name: "Formal", instructions: "Make the tone formal and polite."),
    ]
}

extension Array where Element == AppRule {
    /// The enabled rules with instructions for the app, in list order.
    public func matching(_ bundleID: String?) -> [AppRule] {
        guard let bundleID else {
            return []
        }
        return filter { rule in
            rule.enabled && !rule.instructions.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                && rule.apps.contains { $0.bundleID == bundleID }
        }
    }
}

/// What the model gets for a dictation that several rules apply to, or "" for none.
public func combinedInstructions(_ rules: [AppRule]) -> String {
    rules.map { $0.instructions.trimmingCharacters(in: .whitespacesAndNewlines) }.joined(separator: "\n\n")
}
