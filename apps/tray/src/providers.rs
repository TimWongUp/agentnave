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

/// The host a row's connections run as; its service knows no other names.
pub(crate) fn boss_of(id: &str) -> &str {
    match id {
        "claude-desktop" => "claude",
        id => id.split_once('@').map_or(id, |(boss, _)| boss),
    }
}
