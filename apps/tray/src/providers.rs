pub const PROVIDERS: [&str; 9] = [
    "zcode",
    "antigravity",
    "claude",
    "codebuddy",
    "codex",
    "deepseek",
    "devin",
    "grok",
    "pi",
];

/// What each CLI is called in rules text, the same names the settings page shows.
const NAMES: [(&str, &str); 9] = [
    ("codex", "Codex CLI"),
    ("claude", "Claude Code"),
    ("codebuddy", "CodeBuddy Code"),
    ("grok", "Grok CLI"),
    ("antigravity", "Antigravity CLI"),
    ("pi", "Pi Coding Agent"),
    ("deepseek", "DeepSeek Harness"),
    ("devin", "Devin CLI"),
    ("zcode", "ZCode"),
];

pub(crate) fn display_name(provider: &str) -> Option<&'static str> {
    let named = NAMES.iter().find(|(id, _)| *id == provider);
    named.map(|(_, name)| *name)
}

/// The host a row's connections run as; its service knows no other names.
pub(crate) fn boss_of(id: &str) -> &str {
    match id {
        "claude-desktop" => "claude",
        id => id.split_once('@').map_or(id, |(boss, _)| boss),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settings page keeps its own copy in `PROVIDER_NAMES`; the two must not drift.
    #[test]
    fn display_names_match_the_settings_page() {
        let page = include_str!("../ui/settings-state.js");
        let (_, rest) = page.split_once("const PROVIDER_NAMES = {").expect("table");
        let (body, _) = rest.split_once("};").expect("end of table");
        let shown: Vec<(&str, &str)> = body
            .lines()
            .filter_map(|line| line.trim().trim_end_matches(',').split_once(':'))
            .map(|(id, name)| (id.trim(), name.trim().trim_matches('"')))
            .collect();
        assert_eq!(shown, NAMES);
        for provider in PROVIDERS {
            assert!(display_name(provider).is_some(), "{provider}");
        }
    }
}
