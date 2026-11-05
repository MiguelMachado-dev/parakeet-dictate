import DictateCore
import SwiftUI

struct ReplacementRow: Identifiable {
    let id = UUID()
    var source = ""
    var target = ""
}

final class DictionaryEditor: ObservableObject {
    @Published var words: String { didSet { status = "" } }
    @Published var rows: [ReplacementRow] { didSet { status = "" } }
    @Published var sample = "O PY é o harness."
    @Published var status = ""
    @Published var saved: PersonalDictionary

    init(_ dictionary: PersonalDictionary) {
        saved = dictionary
        words = dictionary.wordsText
        rows = dictionary.replacements.map { ReplacementRow(source: $0.source, target: $0.target) }
        if rows.isEmpty { rows = [ReplacementRow()] }
    }

    var parsed: Result<PersonalDictionary, Error> {
        Result {
            var replacements: [TextReplacement] = []
            for (index, row) in rows.enumerated() {
                let source = row.source.trimmingCharacters(in: .whitespacesAndNewlines)
                let target = row.target.trimmingCharacters(in: .whitespacesAndNewlines)
                if source.isEmpty && target.isEmpty { continue }
                guard !source.isEmpty, !target.isEmpty else {
                    throw DictionaryError("Replacement row \(index + 1): enter both original and replacement.")
                }
                replacements.append(TextReplacement(source: source, target: target))
            }
            return try PersonalDictionary.validated(words: words, replacements: replacements)
        }
    }

    func addRow(after id: ReplacementRow.ID) {
        guard let index = rows.firstIndex(where: { $0.id == id }) else { return }
        rows.insert(ReplacementRow(), at: index + 1)
    }

    func removeRow(_ id: ReplacementRow.ID) {
        rows.removeAll { $0.id == id }
        if rows.isEmpty { rows = [ReplacementRow()] }
    }

    func addExample() {
        let existing = words.components(separatedBy: .newlines).map { $0.trimmingCharacters(in: .whitespaces).lowercased() }
        for word in ["PI", "harness"] where !existing.contains(word.lowercased()) {
            words += (words.isEmpty || words.hasSuffix("\n") ? "" : "\n") + word
        }
        let sources = rows.map { $0.source.trimmingCharacters(in: .whitespaces).lowercased() }
        if !sources.contains("py") {
            let example = ReplacementRow(source: "PY", target: "PI")
            if let index = rows.firstIndex(where: { $0.source.isEmpty && $0.target.isEmpty }) {
                rows[index] = example
            } else {
                rows.append(example)
            }
        }
        status = ""
    }
}

struct DictionaryView: View {
    @ObservedObject var editor: DictionaryEditor
    let onSave: (PersonalDictionary) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Preferred spellings").font(.headline)
            Text("One word or phrase per line. Used as vocabulary during AI cleanup and to preserve spelling before pasting.")
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            TextEditor(text: $editor.words)
                .font(.body).border(.secondary.opacity(0.3)).frame(height: 100)
                .accessibilityLabel("Preferred spellings")
            Text("Replacements").font(.headline)
            Text("Enter the original and its replacement. Use + to add a row. Matches ignore case and replace whole words or phrases, not parts of identifiers or file paths.")
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            ScrollView {
                VStack(spacing: 8) {
                    ForEach($editor.rows) { row in
                        HStack(spacing: 8) {
                            TextField("Original", text: row.source)
                                .accessibilityLabel("Original")
                            TextField("Replace with", text: row.target)
                                .accessibilityLabel("Replace with")
                            if editor.rows.count > 1 || !row.wrappedValue.source.isEmpty || !row.wrappedValue.target.isEmpty {
                                Button { editor.removeRow(row.wrappedValue.id) } label: {
                                    Image(systemName: "minus")
                                }
                                .help("Remove replacement")
                                .accessibilityLabel("Remove replacement")
                            }
                            Button { editor.addRow(after: row.wrappedValue.id) } label: {
                                Image(systemName: "plus")
                            }
                            .help("Add replacement")
                            .accessibilityLabel("Add replacement")
                        }
                    }
                }
                .padding(2)
            }
            .frame(height: min(CGFloat(editor.rows.count) * 34, 170))
            HStack {
                Button("Add PI / harness example") { editor.addExample() }
                Spacer()
            }
            Divider()
            HStack {
                Text("Try it")
                TextField("Text to preview", text: $editor.sample)
            }
            switch editor.parsed {
            case .success(let dictionary):
                let preview = dictionary.apply(editor.sample)
                Text(preview.isEmpty ? "Preview appears here." : preview)
                    .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("dictionary-preview")
            case .failure(let error):
                Text(String(describing: error)).foregroundStyle(.red).font(.caption)
            }
            Text("Replacements run locally after AI cleanup, and also when cleanup is off, skipped, or fails. Inserted text is not replaced again. The same dictionary applies when transcribing on the PC.")
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Text(editor.status).font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Save Dictionary") {
                    if case .success(let dictionary) = editor.parsed {
                        onSave(dictionary)
                        editor.saved = dictionary
                        editor.status = "Saved. Applies to the next dictation."
                    }
                }
                .disabled(!canSave)
            }
        }
        .padding(20).frame(width: 540)
    }

    private var canSave: Bool {
        if case .success(let dictionary) = editor.parsed { return dictionary != editor.saved }
        return false
    }
}
