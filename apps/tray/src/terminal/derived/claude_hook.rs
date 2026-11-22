//! Claude Code `permission_fallback` validation, `UserPromptSubmit` hook, and cleanup (ADR-0059).

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::native_logs;

const MODES: [&str; 2] = ["auto", "bypassPermissions"];

pub(super) fn claude_permission_modes(
    permissions: &Value,
) -> Result<(Option<&str>, Option<&str>), String> {
    let mode_val = permissions.get("permission_mode");
    let mode = match mode_val {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => return Err("permission_mode must be a string".to_owned()),
    };
    let fb_val = permissions.get("permission_fallback");
    let fallback = match fb_val {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => {
            return Err(
                "permission_fallback must be auto or bypassPermissions, with permission_mode set to the other one".to_owned(),
            );
        }
    };
    if let Some(fb) = fallback {
        let valid_pair = MODES.contains(&fb) && mode.is_some_and(|m| MODES.contains(&m) && m != fb);
        if !valid_pair {
            return Err(
                "permission_fallback must be auto or bypassPermissions, with permission_mode set to the other one".to_owned(),
            );
        }
    }
    Ok((mode, fallback))
}

pub(super) fn prepare_mode_hook() -> Result<tempfile::TempDir, String> {
    let dir = tempfile::Builder::new()
        .prefix("agentnave-claude-hook-")
        .tempdir()
        .map_err(|e| format!("无法创建 Claude hook 目录：{e}"))?;
    fs::write(dir.path().join("armed"), b"1")
        .map_err(|e| format!("无法初始化 Claude hook 标记：{e}"))?;
    Ok(dir)
}

fn shell_single_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\\''"))
}

pub(super) fn hook_settings(hook_dir: &Path) -> Value {
    let armed = shell_single_quote(&hook_dir.join("armed").display().to_string());
    let mode_file = shell_single_quote(&hook_dir.join("mode").display().to_string());
    let cmd = format!(
        "[ -f {armed} ] || exit 0; rm -f {armed}; \
        /usr/bin/perl -MJSON::PP -0777 -e 'my $d = eval {{ decode_json(<STDIN>) }}; \
        my $m = ref($d) eq \"HASH\" ? ($d->{{permission_mode}} // \"\") : \"\"; \
        my $tmp = \"$ARGV[0].tmp.$$\"; \
        open my $fh, \">\", $tmp or die $!; print {{$fh}} $m; close $fh or die $!; \
        rename $tmp, $ARGV[0] or die $!; \
        if ($m eq \"default\") {{ print \"{{\\\"decision\\\":\\\"block\\\",\\\"reason\\\":\\\"AgentNave restarts Claude Code in its fallback permission mode for this prompt\\\"}}\\n\"; }}' {mode_file}"
    );
    json!([{
        "hooks": [{
            "type": "command",
            "command": cmd,
        }]
    }])
}

pub(super) fn read_observed_mode(hook_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(hook_dir.join("mode")).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

pub(super) fn drop_empty_claude_session(homes: &[PathBuf], session_id: &str) {
    let Some(path) = native_logs::claude_transcript_path(homes, session_id) else {
        return;
    };
    let Ok(file) = File::open(&path) else {
        return;
    };
    let has_turn = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| {
            serde_json::from_str::<Value>(&line).is_ok_and(|row| {
                row["type"].as_str() == Some("assistant")
                    || (row["type"].as_str() == Some("system")
                        && row["subtype"].as_str() == Some("turn_duration"))
            })
        });
    if !has_turn {
        let _ = fs::remove_file(path);
    }
}
