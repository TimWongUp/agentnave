pub const PROVIDERS: [&str; 8] = [
    "zcode",
    "antigravity",
    "claude",
    "codebuddy",
    "codex",
    "deepseek",
    "grok",
    "pi",
];
/// Providers whose host defaults to `launch` rather than `subagent`, matching
/// `ProviderAdapter.default_host_cli_mode` on the Python side.
pub const LAUNCH_HOST_CLIS: [&str; 1] = ["antigravity"];

/// The host a row's connections run as; its service knows no other names. Mirrored by `_row_cli`
/// in src/agentnave/t3_seats.py.
pub(crate) fn boss_of(id: &str) -> &str {
    match id {
        "claude-desktop" => "claude",
        id => id.split_once('@').map_or(id, |(boss, _)| boss),
    }
}

/// The provider a boss runs as its own CLI; WorkBuddy shares CodeBuddy's.
pub(crate) fn own_provider(boss: &str) -> &str {
    match boss {
        "workbuddy" => "codebuddy",
        boss => boss,
    }
}

/// What a boss row does with tasks for its own CLI when `orchestration.json` omits the row.
pub(crate) fn default_host_cli(id: &str) -> &'static str {
    let provider = own_provider(boss_of(id));
    if LAUNCH_HOST_CLIS.contains(&provider) {
        "launch"
    } else {
        "subagent"
    }
}
