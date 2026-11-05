import Foundation
import Testing
@testable import DictateCore

private struct DictionaryCase: Decodable {
    var words: [String]
    var replacements: [TextReplacement]
    var input: String
    var output: String
}

@Test func dictionaryMatchesWindowsFixtures() throws {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
    let data = try Data(contentsOf: root.appendingPathComponent("tests/fixtures/dictionary.json"))
    for item in try JSONDecoder().decode([DictionaryCase].self, from: data) {
        let dictionary = PersonalDictionary(words: item.words, replacements: item.replacements)
        #expect(dictionary.apply(item.input) == item.output)
        #expect(try JSONDecoder().decode(PersonalDictionary.self, from: JSONEncoder().encode(dictionary)) == dictionary)
    }
}

@Test func dictionaryEditorRoundTripAndInstructions() throws {
    let dictionary = try PersonalDictionary.parse(words: "\nPI\nharness\n João \n", replacements: "\n PY => PI\nmeu link => https://example.com/?a=>b\n")
    #expect(try PersonalDictionary.parse(words: dictionary.wordsText, replacements: dictionary.replacementsText) == dictionary)
    let instructions = dictionary.instructions(appInstructions: "Translate to English.")
    #expect(instructions.hasPrefix("Translate to English.\n\nPersonal dictionary"))
    #expect(instructions.contains("harness"))
    #expect(PersonalDictionary().instructions(appInstructions: "unchanged") == "unchanged")
}

@Test func dictionaryRejectsAmbiguousOrIncompleteEntries() {
    for (words, replacements) in [("PI\npi", ""), ("", "py => PI\nPY => Python"), ("", "PY"),
                                   ("", "PY =>"), ("", "=> PI"), ("a\0b", ""), ("", "a => b\0c")] {
        #expect(throws: DictionaryError.self) { try PersonalDictionary.parse(words: words, replacements: replacements) }
    }
}

@Test func dictionaryValidatesStructuredReplacementFields() throws {
    let dictionary = try PersonalDictionary.validated(words: "PI\nharness", replacements: [
        TextReplacement(source: " PY ", target: " PI "),
        TextReplacement(source: "link", target: "https://example.com/?a=>b"),
    ])
    #expect(dictionary.apply("PY link") == "PI https://example.com/?a=>b")
    for entry in [TextReplacement(source: "PY", target: ""), TextReplacement(source: "PY", target: "PI\nother"),
                  TextReplacement(source: "=>", target: "arrow")] {
        #expect(throws: DictionaryError.self) { try PersonalDictionary.validated(words: "", replacements: [entry]) }
    }
}
