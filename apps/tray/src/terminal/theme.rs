//! Which theme a CLI starts in, so it reads well on the terminal's skin (ADR-0103). Codex, Pi,
//! Devin and Antigravity ask the terminal for its background and follow it; these three draw
//! their own colours and are told the App's light or dark theme instead. A theme the user chose
//! in the CLI's own config is left alone.

use std::path::{Path, PathBuf};

/// Environment variables and arguments to add to the CLI's launch.
pub(super) struct Theme {
    pub env: Vec<(&'static str, &'static str)>,
    pub args: Vec<String>,
}

pub(super) fn for_cli(provider: &str, light: bool) -> Theme {
    let mut theme = Theme {
        env: Vec::new(),
        args: Vec::new(),
    };
    let Some(home) = dirs::home_dir() else {
        return theme;
    };
    let name = if light { "light" } else { "dark" };
    match provider {
        // Grok paints its own background and never asks the terminal for it.
        "grok" if !toml_sets_theme(&home.join(".grok/config.toml")) => {
            theme
                .env
                .push(("GROK_THEME", if light { "grok-day" } else { "grok-night" }));
        }
        // Claude Code and CodeBuddy keep their dark palette on any background unless told.
        "claude"
            if !json_sets_theme(&[
                home.join(".claude.json"),
                home.join(".claude/settings.json"),
            ]) =>
        {
            theme.args = settings(name);
        }
        "codebuddy" if !json_sets_theme(&[home.join(".codebuddy/settings.json")]) => {
            theme.args = settings(name);
        }
        _ => {}
    }
    theme
}

fn settings(name: &str) -> Vec<String> {
    vec!["--settings".to_owned(), format!(r#"{{"theme":"{name}"}}"#)]
}

fn toml_sets_theme(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| {
        text.lines().any(|line| {
            line.trim_start()
                .strip_prefix("theme")
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
    })
}

fn json_sets_theme(paths: &[PathBuf]) -> bool {
    paths.iter().any(|path| {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|config| config.get("theme").is_some())
    })
}
