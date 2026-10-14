from parakeet_dictate.app_rules import AppRule, combined_instructions


def test_enabled_nonempty_rules_match_executables_in_order_without_case_sensitivity():
    rules = (
        AppRule("English", "Translate to English.", ("Slack.exe", "outlook.exe")),
        AppRule("Empty", " \n", ("slack.exe",)),
        AppRule("Off", "Shout.", ("slack.exe",), enabled=False),
        AppRule("Casual", " Keep it casual. \n", ("SLACK.EXE", "slack.exe")),
        AppRule("Formal", "Be formal.", ("outlook.exe",)),
    )
    assert combined_instructions(rules, "sLaCk.ExE") == "Translate to English.\n\nKeep it casual."
    assert combined_instructions(rules, "OUTLOOK.EXE") == "Translate to English.\n\nBe formal."
    assert combined_instructions(rules, "notepad.exe") == ""
    assert combined_instructions(rules, None) == ""
    assert combined_instructions((), "slack.exe") == ""


def test_apps_match_whole_executable_names_and_no_apps_means_no_match():
    rules = (AppRule("English", "Translate.", ("slack.exe",)), AppRule("All?", "Shout."))
    assert combined_instructions(rules, "not-slack.exe") == ""
    assert combined_instructions(rules, "slack") == ""
    assert combined_instructions(rules, "") == ""
