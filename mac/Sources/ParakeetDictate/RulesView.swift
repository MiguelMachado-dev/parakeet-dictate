import AppKit
import DictateCore
import SwiftUI
import UniformTypeIdentifiers

/// The rules window's transient state. A class for the same reason as ConnectionTest.
final class RuleEditor: ObservableObject {
    @Published var selection: AppRule.ID?
    /// What "Add App" offers first: the apps running now, which include the one to dictate into.
    @Published private(set) var running: [AppRule.App] = []
    private var observers: [NSObjectProtocol] = []

    init() {
        refresh()
        let center = NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.didLaunchApplicationNotification, NSWorkspace.didTerminateApplicationNotification] {
            observers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in self?.refresh() })
        }
    }

    deinit {
        observers.forEach(NSWorkspace.shared.notificationCenter.removeObserver)
    }

    private func refresh() {
        var seen: Set<String> = [Bundle.main.bundleIdentifier ?? ""]
        running = NSWorkspace.shared.runningApplications
            .filter { $0.activationPolicy == .regular }
            .compactMap { app in
                guard let id = app.bundleIdentifier, seen.insert(id).inserted else {
                    return nil
                }
                return AppRule.App(bundleID: id, name: app.localizedName ?? id)
            }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }
}

struct RulesView: View {
    @ObservedObject var settings: Settings
    @ObservedObject var editor: RuleEditor

    var body: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                List(selection: $editor.selection) {
                    ForEach(settings.rules) { rule in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(rule.name.isEmpty ? "Untitled" : rule.name)
                            Text(rule.apps.isEmpty ? "No apps" : rule.apps.map(\.name).joined(separator: ", "))
                                .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                        .opacity(rule.enabled ? 1 : 0.5)
                        .tag(rule.id)
                    }
                    .onMove { settings.rules.move(fromOffsets: $0, toOffset: $1) }
                }
                Divider()
                HStack(spacing: 0) {
                    Menu {
                        ForEach(AppRule.presets) { preset in
                            Button(preset.name) { add(preset) }
                        }
                        Divider()
                        Button("Blank Rule") { add(AppRule(name: "New rule", instructions: "")) }
                    } label: {
                        Image(systemName: "plus")
                    }
                    .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                    .frame(width: 28, height: 24)
                    .help("Add a rule")
                    Button {
                        settings.rules.removeAll { $0.id == editor.selection }
                        editor.selection = settings.rules.first?.id
                    } label: {
                        Image(systemName: "minus")
                    }
                    .buttonStyle(.borderless)
                    .frame(width: 28, height: 24)
                    .disabled(editor.selection == nil)
                    .help("Delete the rule")
                    Spacer()
                }
                .padding(.horizontal, 4)
            }
            .frame(width: 200)
            Divider()
            if let id = editor.selection, settings.rules.contains(where: { $0.id == id }) {
                RuleDetail(rule: binding(id), running: editor.running)
            } else {
                Text(settings.rules.isEmpty ? "Add a rule with +." : "Select a rule.")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(width: 660, height: 440)
    }

    private func add(_ preset: AppRule) {
        var rule = preset
        rule.id = UUID()
        settings.rules.append(rule)
        editor.selection = rule.id
    }

    /// Looks the rule up by id on every access, so a binding left over from a deleted rule never indexes past the end.
    private func binding(_ id: AppRule.ID) -> Binding<AppRule> {
        Binding(
            get: { settings.rules.first { $0.id == id } ?? AppRule(name: "", instructions: "") },
            set: { rule in
                if let index = settings.rules.firstIndex(where: { $0.id == id }) {
                    settings.rules[index] = rule
                }
            }
        )
    }
}

/// A plain Binding rather than @Binding, which, like @State, may need Xcode's macro plugins.
private struct RuleDetail: View {
    let rule: Binding<AppRule>
    let running: [AppRule.App]

    var body: some View {
        Form {
            Section {
                TextField("Name", text: rule.name)
                Toggle("On", isOn: rule.enabled)
            }
            Section {
                TextField("Instructions", text: rule.instructions, prompt: Text("Translate to English."), axis: .vertical)
                    .lineLimit(3...8)
                    .labelsHidden()
            } header: {
                Text("Instructions")
            } footer: {
                Text(
                    "The model follows these after its usual cleanup, even when they ask it to translate or restyle. "
                        + "A reply much longer than what you said is rejected, and the raw transcript is pasted."
                )
                .font(.caption).foregroundStyle(.secondary)
            }
            Section {
                ForEach(rule.wrappedValue.apps, id: \.bundleID) { app in
                    HStack {
                        Image(nsImage: icon(app.bundleID))
                        Text(app.name)
                        Spacer()
                        Button {
                            rule.wrappedValue.apps.removeAll { $0 == app }
                        } label: {
                            Image(systemName: "minus.circle")
                        }
                        .buttonStyle(.borderless)
                        .help("Remove \(app.name)")
                    }
                }
                Menu("Add App") {
                    let chosen = Set(rule.wrappedValue.apps.map(\.bundleID))
                    ForEach(running.filter { !chosen.contains($0.bundleID) }, id: \.bundleID) { app in
                        Button(app.name) { rule.wrappedValue.apps.append(app) }
                    }
                    Divider()
                    Button("Choose from Applications…") { chooseApps() }
                }
                .fixedSize()
            } header: {
                Text("Apps")
            } footer: {
                Text("The rule applies when one of these apps is in front as a dictation ends.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }

    private func icon(_ bundleID: String) -> NSImage {
        let image = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID)
            .map { NSWorkspace.shared.icon(forFile: $0.path) } ?? NSImage(systemSymbolName: "app", accessibilityDescription: nil)!
        image.size = NSSize(width: 16, height: 16)
        return image
    }

    private func chooseApps() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.application]
        panel.allowsMultipleSelection = true
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        guard panel.runModal() == .OK else {
            return
        }
        for url in panel.urls {
            guard let id = Bundle(url: url)?.bundleIdentifier, !rule.wrappedValue.apps.contains(where: { $0.bundleID == id }) else {
                continue
            }
            var name = FileManager.default.displayName(atPath: url.path)
            if name.hasSuffix(".app") {
                name.removeLast(4)
            }
            rule.wrappedValue.apps.append(AppRule.App(bundleID: id, name: name))
        }
    }
}
