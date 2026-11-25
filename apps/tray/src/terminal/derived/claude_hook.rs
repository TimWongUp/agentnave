//! Claude Code hooks (`Stop` / `StopFailure` permanent hooks and `permission_fallback` `UserPromptSubmit`).

use std::fs;
use std::io::{BufReader, Read};
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

/// Create the per-derived-terminal hook directory (lives for the terminal's lifetime).
/// When `arm_mode_fallback` is true, also arms the one-shot `UserPromptSubmit` mode check.
pub(super) fn prepare_claude_hook(arm_mode_fallback: bool) -> Result<tempfile::TempDir, String> {
    let dir = tempfile::Builder::new()
        .prefix("agentnave-claude-hook-")
        .tempdir()
        .map_err(|e| format!("无法创建 Claude hook 目录：{e}"))?;
    if arm_mode_fallback {
        fs::write(dir.path().join("armed"), b"1")
            .map_err(|e| format!("无法初始化 Claude hook 标记：{e}"))?;
    }
    Ok(dir)
}

#[cfg(test)]
pub(super) fn prepare_mode_hook() -> Result<tempfile::TempDir, String> {
    prepare_claude_hook(true)
}

pub(in crate::terminal) fn events_path(hook_dir: &Path) -> PathBuf {
    hook_dir.join("events.jsonl")
}

fn shell_single_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\\''"))
}

pub(super) fn hook_settings(hook_dir: &Path) -> Value {
    let armed = shell_single_quote(&hook_dir.join("armed").display().to_string());
    let mode_file = shell_single_quote(&hook_dir.join("mode").display().to_string());
    let cmd = format!(
        "[ -f {armed} ] || exit 0; rm -f {armed}; \
        /usr/bin/perl -MJSON::PP -e 'my $raw = \"\"; my $n = read(STDIN, $raw, 1048577); \
        my $m = \"default\"; \
        if (defined($n) && $n <= 1048576) {{ \
            my $d = eval {{ decode_json($raw) }}; \
            $m = ref($d) eq \"HASH\" ? ($d->{{permission_mode}} // \"\") : \"\"; \
        }} \
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

/// Permanent (non-one-shot) command hook for `Stop` and `StopFailure`.
/// Appends compact JSON lines to `<hook_dir>/events.jsonl` while preserving whether
/// `background_tasks` was present vs absent in stdin.
pub(super) fn stop_hook_settings(hook_dir: &Path) -> Value {
    let events_file = shell_single_quote(&events_path(hook_dir).display().to_string());
    let cmd = format!(
        "/usr/bin/perl -MFcntl=:DEFAULT,:flock -MJSON::PP -e '\
        my $raw = \"\"; my $n = read(STDIN, $raw, 1048577); \
        exit 0 if !defined($n) || $n > 1048576; \
        my $d = eval {{ decode_json($raw) }}; \
        exit 0 unless ref($d) eq \"HASH\"; \
        my $ev = $d->{{hook_event_name}} // \"\"; \
        exit 0 unless $ev eq \"Stop\" || $ev eq \"StopFailure\"; \
        my %out = (hook_event_name => $ev); \
        for my $k (qw(session_id prompt_id stop_hook_active error error_details)) {{ \
            $out{{$k}} = $d->{{$k}} if exists $d->{{$k}}; \
        }} \
        if ($ev eq \"StopFailure\" && defined $d->{{last_assistant_message}}) {{ \
            $out{{last_assistant_message}} = substr(\"$d->{{last_assistant_message}}\", 0, 400); \
        }} \
        if (exists $d->{{background_tasks}}) {{ \
            $out{{background_tasks}} = $d->{{background_tasks}}; \
        }} \
        exit 0 if -l $ARGV[0]; \
        my $flags = O_WRONLY | O_CREAT | O_APPEND | (eval {{ O_NOFOLLOW() }} // 0) | (eval {{ O_NONBLOCK() }} // 0); \
        sysopen(my $fh, $ARGV[0], $flags, 0600) or exit 0; \
        exit 0 unless -f $fh; \
        flock($fh, 2); \
        print {{$fh}} encode_json(\\%out) . \"\\n\"; \
        close $fh;' {events_file}"
    );
    json!([{
        "hooks": [{
            "type": "command",
            "command": cmd,
        }]
    }])
}

pub(super) fn build_claude_hooks(hook_dir: &Path, include_mode_fallback: bool) -> Value {
    let mut hooks = serde_json::Map::new();
    let stop = stop_hook_settings(hook_dir);
    hooks.insert("Stop".to_owned(), stop.clone());
    hooks.insert("StopFailure".to_owned(), stop);
    if include_mode_fallback {
        hooks.insert("UserPromptSubmit".to_owned(), hook_settings(hook_dir));
    }
    Value::Object(hooks)
}

pub(super) fn read_observed_mode(hook_dir: &Path) -> Option<String> {
    let file = native_logs::open_regular_readonly(&hook_dir.join("mode"))?;
    let mut text = String::new();
    file.take(1024).read_to_string(&mut text).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

pub(super) fn drop_empty_claude_session(homes: &[PathBuf], session_id: &str) {
    let Some(path) = native_logs::claude_transcript_path(homes, session_id) else {
        return;
    };
    let Some(file) = native_logs::open_regular_readonly(&path) else {
        return;
    };
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut has_turn = false;
    loop {
        match native_logs::read_bounded_line(&mut reader, &mut line) {
            native_logs::TranscriptLine::Eof => break,
            native_logs::TranscriptLine::Oversized => {
                has_turn = true;
                break;
            }
            native_logs::TranscriptLine::Line => {}
        }
        if serde_json::from_str::<Value>(&line).is_ok_and(|row| {
            row["type"].as_str() == Some("assistant")
                || (row["type"].as_str() == Some("system")
                    && row["subtype"].as_str() == Some("turn_duration"))
        }) {
            has_turn = true;
            break;
        }
    }
    if !has_turn
        && fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_symlink() && m.is_file())
    {
        let _ = fs::remove_file(path);
    }
}
