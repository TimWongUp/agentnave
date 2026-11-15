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
/// Providers whose host defaults to `launch` rather than `subagent`, matching
/// `ProviderAdapter.default_host_cli_mode` on the Python side.
pub const LAUNCH_HOST_CLIS: [&str; 1] = ["antigravity"];

/// The host a row's connections run as; its service knows no other names.
pub(crate) fn boss_of(id: &str) -> &str {
    match id {
        "claude-desktop" => "claude",
        id => id.split_once('@').map_or(id, |(boss, _)| boss),
    }
}

/// What a boss row does with tasks for its own CLI when `orchestration.json` omits the row.
pub(crate) fn default_host_cli(id: &str) -> &'static str {
    let provider = match boss_of(id) {
        "workbuddy" => "codebuddy",
        boss => boss,
    };
    if LAUNCH_HOST_CLIS.contains(&provider) {
        "launch"
    } else {
        "subagent"
    }
}
