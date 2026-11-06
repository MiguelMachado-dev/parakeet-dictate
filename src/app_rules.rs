use crate::text::casefold;

#[derive(Clone, PartialEq, Eq)]
pub struct AppRule {
    pub name: String,
    pub instructions: String,
    pub apps: Vec<String>,
    pub enabled: bool,
}

impl AppRule {
    pub fn new(name: &str, instructions: &str, apps: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            instructions: instructions.to_owned(),
            apps: apps.iter().map(|app| (*app).to_owned()).collect(),
            enabled: true,
        }
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

impl std::fmt::Debug for AppRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppRule").field("name", &self.name).field("apps", &self.apps).field("enabled", &self.enabled).finish()
    }
}

/// Enabled, non-empty rules for this executable, in config order.
pub fn combined_instructions(rules: &[AppRule], executable: Option<&str>) -> String {
    let Some(executable) = executable else {
        return String::new();
    };
    let wanted = casefold(executable);
    rules
        .iter()
        .filter(|rule| rule.enabled && !rule.instructions.trim().is_empty() && rule.apps.iter().any(|app| casefold(app) == wanted))
        .map(|rule| rule.instructions.trim())
        .collect::<Vec<_>>()
        .join("\n\n")
}
