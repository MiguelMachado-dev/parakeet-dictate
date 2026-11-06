use parakeet_dictate::app_rules::{AppRule, combined_instructions};

#[test]
fn test_enabled_nonempty_rules_match_executables_in_order_without_case_sensitivity() {
    let rules = [
        AppRule::new("English", "Translate to English.", &["Slack.exe", "outlook.exe"]),
        AppRule::new("Empty", " \n", &["slack.exe"]),
        AppRule::new("Off", "Shout.", &["slack.exe"]).disabled(),
        AppRule::new("Casual", " Keep it casual. \n", &["SLACK.EXE", "slack.exe"]),
        AppRule::new("Formal", "Be formal.", &["outlook.exe"]),
    ];
    assert_eq!(combined_instructions(&rules, Some("sLaCk.ExE")), "Translate to English.\n\nKeep it casual.");
    assert_eq!(combined_instructions(&rules, Some("OUTLOOK.EXE")), "Translate to English.\n\nBe formal.");
    assert_eq!(combined_instructions(&rules, Some("notepad.exe")), "");
    assert_eq!(combined_instructions(&rules, None), "");
    assert_eq!(combined_instructions(&[], Some("slack.exe")), "");
}

#[test]
fn test_apps_match_whole_executable_names_and_no_apps_means_no_match() {
    let rules = [AppRule::new("English", "Translate.", &["slack.exe"]), AppRule::new("All?", "Shout.", &[])];
    assert_eq!(combined_instructions(&rules, Some("not-slack.exe")), "");
    assert_eq!(combined_instructions(&rules, Some("slack")), "");
    assert_eq!(combined_instructions(&rules, Some("")), "");
}
