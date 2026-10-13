import Foundation
import Testing

@testable import DictateCore

private let slack = AppRule.App(bundleID: "com.tinyspeck.slackmacgap", name: "Slack")
private let mail = AppRule.App(bundleID: "com.apple.mail", name: "Mail")

@Test func rulesMatchTheirAppsInListOrder() {
    var off = AppRule(name: "Off", instructions: "Shout.", apps: [slack])
    off.enabled = false
    let rules = [
        AppRule(name: "English", instructions: "Translate to English.", apps: [slack, mail]),
        AppRule(name: "Empty", instructions: " \n", apps: [slack]),
        off,
        AppRule(name: "Casual", instructions: " Keep it casual. ", apps: [slack]),
        AppRule(name: "Formal", instructions: "Be formal.", apps: [mail]),
    ]
    #expect(rules.matching(slack.bundleID).map(\.name) == ["English", "Casual"])
    #expect(combinedInstructions(rules.matching(slack.bundleID)) == "Translate to English.\n\nKeep it casual.")
    #expect(rules.matching(mail.bundleID).map(\.name) == ["English", "Formal"])
    #expect(rules.matching("com.apple.Terminal").isEmpty)
    #expect(rules.matching(nil).isEmpty)
    #expect(combinedInstructions([]) == "")
}

@Test func rulesSurviveASaveAndLoad() throws {
    var rule = AppRule(name: "English", instructions: "Translate to English.", apps: [slack])
    rule.enabled = false
    let data = try JSONEncoder().encode([rule])
    #expect(try JSONDecoder().decode([AppRule].self, from: data) == [rule])
}
