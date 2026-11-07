//! CLI maintenance belongs to the App and follows the detected installation (ADR-0055).
mod codex;
mod install;
mod latest;
mod locks;
#[cfg(test)]
mod tests;

use super::same_path;
use latest::{Channel, cached, newer};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

static UPDATING: LazyLock<locks::Updates> = LazyLock::new(locks::Updates::default);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug)]
struct Plan {
    program: PathBuf,
    args: Vec<String>,
    command: String,
    via: &'static str,
    channel: Channel,
    environment: locks::Environment,
}

pub(crate) fn state() -> Value {
    snapshot(&super::detected(false), cached)
}

pub(crate) fn install_commands() -> Value {
    install::commands(std::env::consts::OS)
}

fn snapshot(all: &Value, query: impl Fn(&Channel) -> Option<String> + Sync) -> Value {
    let clis: serde_json::Map<String, Value> = std::thread::scope(|scope| {
        let probes: Vec<_> = crate::members::PROVIDERS
            .into_iter()
            .filter(|provider| all[provider]["installed"] == true)
            .map(|provider| {
                let query = &query;
                (
                    provider,
                    scope.spawn(move || {
                        if provider == "codex" {
                            codex::state(&all[provider], &codex::paths(&all[provider]), query)
                        } else {
                            cli_state(provider, &all[provider], query)
                        }
                    }),
                )
            })
            .collect();
        probes
            .into_iter()
            .map(|(provider, probe)| {
                let state = probe.join().unwrap_or_else(|_| {
                    json!({
                        "version": all[provider]["version"], "latest": null,
                        "update": false, "via": null, "command": null
                    })
                });
                (provider.to_owned(), state)
            })
            .collect()
    });
    json!({"clis": clis})
}

fn cli_state(provider: &str, info: &Value, query: impl Fn(&Channel) -> Option<String>) -> Value {
    let plan = info["path"]
        .as_str()
        .and_then(|p| install::identify(provider, Path::new(p)));
    let latest = plan.as_ref().and_then(|plan| query(&plan.channel));
    let available = latest
        .as_deref()
        .zip(info["version"].as_str())
        .is_some_and(|(a, b)| newer(a, b));
    json!({"version": info["version"], "latest": latest, "update": available,
        "via": plan.as_ref().map(|p| p.via), "command": plan.as_ref().map(|p| &p.command)})
}

/// Only a Codex standalone installation can be selected; paths never come from a request.
pub(crate) fn validate_target(
    provider: &str,
    target: Option<&Value>,
) -> Result<Option<&'static str>, String> {
    match target {
        None => Ok(None),
        Some(Value::String(target)) if provider == "codex" && target == "standalone" => {
            Ok(Some("standalone"))
        }
        _ => Err("target 仅支持 Codex 的 standalone；桌面内置 CLI 请更新所属桌面应用".into()),
    }
}

pub(crate) fn update(provider: &str, target: Option<&str>) -> Result<Value, String> {
    if crate::PREVIEW {
        return Err("预览版不执行 CLI 更新".into());
    }
    let path = match (provider, target) {
        ("codex", Some("standalone")) => super::locate::standalone_codex(),
        (_, None) => super::locate::find(match provider {
            "deepseek" => "dsh",
            "antigravity" => "agy",
            "trae" => "traecli",
            other => other,
        }),
        _ => return Err("target 仅支持 Codex 的 standalone".into()),
    }
    .ok_or("未找到此 CLI 安装来源")?;
    update_at(
        provider,
        &path,
        crate::PREVIEW,
        &UPDATING,
        &super::CACHE,
        cached,
        UPDATE_TIMEOUT,
    )?;
    Ok(state())
}

fn read_version(path: &Path) -> Option<String> {
    super::run(path, &["--version"])
        .ok()
        .and_then(|out| super::version_of(&out))
}

fn update_at(
    provider: &str,
    path: &Path,
    preview: bool,
    locks: &locks::Updates,
    cache: &Mutex<super::DetectionCache>,
    query: impl Fn(&Channel) -> Option<String>,
    timeout: Duration,
) -> Result<String, String> {
    let mut claim = locks.claim(preview, provider)?;
    let plan = install::identify(provider, path)
        .ok_or("无法安全更新此 CLI；桌面内置 CLI 请更新所属桌面应用")?;
    claim.environment(&plan.environment);
    // Read and verify this installation directly. A parallel global detection refresh may
    // contain another installation or an old version and must never decide update success.
    let current = read_version(path).ok_or("无法读取当前版本")?;
    let latest = query(&plan.channel).ok_or("无法查询最新版本")?;
    if !newer(&latest, &current) {
        return Err("没有可用的新版".into());
    }
    execute(&plan, timeout)?;
    let after = read_version(path);
    verify(&plan.command, &current, after.as_deref(), &latest)?;
    let after = after.expect("verified version");
    if provider == "codex" {
        codex::remember(path, Some(after.clone()));
    }
    // Also protect in-flight full probes; never wait for model probes or update other paths.
    cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remember(provider, path, &after);
    Ok(after)
}

fn verify(command: &str, before: &str, after: Option<&str>, latest: &str) -> Result<(), String> {
    match after {
        Some(after) if after != before || !newer(latest, after) => Ok(()),
        Some(after) => Err(format!("{command}: 仍是旧版本 {after}（最新 {latest}）")),
        None => Err(format!("{command}: 更新后无法读取版本")),
    }
}

fn execute(plan: &Plan, timeout: Duration) -> Result<(), String> {
    let args: Vec<_> = plan.args.iter().map(String::as_str).collect();
    let mut command = super::probe::command(&plan.program, &args);
    command.stdin(Stdio::null()).env("NO_COLOR", "1");
    let output = crate::process::output(command, timeout).map_err(|error| {
        let detail = match error {
            crate::process::OutputError::Io(e) => e.to_string(),
            crate::process::OutputError::Timeout => "更新超时".into(),
            crate::process::OutputError::Reader => "无法读取更新输出".into(),
        };
        format!("{}: {detail}", plan.command)
    })?;
    if output.status.success() {
        return Ok(());
    }
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let tail = error_tail(&text);
    Err(format!(
        "{}: {}",
        plan.command,
        if tail.is_empty() {
            "更新命令失败"
        } else {
            &tail
        }
    ))
}

fn error_tail(text: &str) -> String {
    // CSI (colors/cursor controls), OSC (titles/hyperlinks), and two-byte escapes.
    let mut clean = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            clean.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.peek() == Some(&'\\')) {
                        if c == '\u{1b}' {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    let lines: Vec<_> = clean
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    lines[lines.len().saturating_sub(3)..].join("\n")
}
