//! Where the boss apps keep their global rules. The App lists and opens them; it never writes them.
//! One file often serves several apps (Grok also reads Claude's, dsh the shared `~/.agents`), so
//! the list is by file, each naming every app that reads it.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// One installed boss app: its id, its name and its own root folder (as the host page resolves it).
pub struct App {
    pub id: &'static str,
    pub label: &'static str,
    pub root: PathBuf,
}

/// The global rule files an app reads, from its own documentation and code: `first` when it reads
/// only the first that exists, otherwise every one that exists.
fn candidates(app: &App, home: &Path) -> (bool, Vec<PathBuf>) {
    let root = &app.root;
    let at = |names: &[&str]| names.iter().map(|name| root.join(name)).collect::<Vec<_>>();
    match app.id {
        "claude" => (false, at(&["CLAUDE.md"])),
        "codex" => (true, at(&["AGENTS.override.md", "AGENTS.md"])),
        "codebuddy" | "workbuddy" => (false, at(&["CODEBUDDY.md"])),
        // Every one of these names counts; `~/.claude` too unless `[compat.claude] agents = false`.
        "grok" => {
            let names = [
                "Agents.md",
                "Claude.md",
                "CLAUDE.md",
                "CLAUDE.local.md",
                "AGENT.md",
                "AGENTS.md",
            ];
            let mut files = at(&names);
            let config = std::fs::read_to_string(root.join("config.toml")).unwrap_or_default();
            let config: toml::Table = config.parse().unwrap_or_default();
            let claude = config.get("compat").and_then(|compat| compat.get("claude"));
            let agents = claude.and_then(|claude| claude.get("agents"));
            if agents.and_then(toml::Value::as_bool) != Some(false) {
                files.extend(names.iter().map(|name| home.join(".claude").join(name)));
            }
            (false, files)
        }
        // agy's root is `~/.gemini/config`; the folder above it counts too.
        "antigravity" => {
            let gemini = root.parent().unwrap_or(root);
            let names = ["GEMINI.md", "AGENTS.md"];
            let mut files: Vec<PathBuf> = names.iter().map(|name| gemini.join(name)).collect();
            files.extend(at(&names));
            (false, files)
        }
        "pi" => (
            true,
            at(&[
                "AGENTS.override.md",
                "AGENTS.md",
                "AGENTS.MD",
                "CLAUDE.md",
                "CLAUDE.MD",
            ]),
        ),
        "deepseek" => {
            let shared = std::env::var_os("DSH_AGENTS_HOME")
                .filter(|dir| !dir.is_empty())
                .map_or_else(|| home.join(".agents"), PathBuf::from);
            (
                false,
                vec![root.join("AGENTS.md"), shared.join("AGENTS.md")],
            )
        }
        "zcode" => (false, at(&["AGENTS.md"])),
        "hermes" => (false, at(&["SOUL.md"])),
        _ => (false, Vec::new()),
    }
}

/// A file under exactly this name: on a case-insensitive disk `Agents.md` is not also `AGENTS.md`.
fn exists(path: &Path) -> bool {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    path.is_file() && entries.into_iter().any(|entry| entry.file_name() == name)
}

/// Each file an app actually reads, or its usual file when it has none yet.
fn files(apps: &[App], home: &Path) -> Vec<(PathBuf, Vec<&'static str>)> {
    let mut out: Vec<(PathBuf, Vec<&'static str>)> = Vec::new();
    for app in apps {
        let (first, candidates) = candidates(app, home);
        let existing: Vec<&PathBuf> = candidates.iter().filter(|path| exists(path)).collect();
        let read: Vec<&PathBuf> = match (first, existing.is_empty()) {
            (_, true) => candidates
                .iter()
                .find(|p| p.ends_with("AGENTS.md"))
                .or(candidates.first())
                .into_iter()
                .collect(),
            (true, false) => existing.into_iter().take(1).collect(),
            (false, false) => existing,
        };
        for path in read {
            match out.iter_mut().find(|(known, _)| known == path) {
                Some((_, readers)) => readers.push(app.label),
                None => out.push((path.clone(), vec![app.label])),
            }
        }
    }
    // Files that exist first, then the ones not created yet.
    out.sort_by_key(|(path, _)| !path.is_file());
    out
}

pub fn list(apps: &[App], home: &Path) -> Value {
    let files: Vec<Value> = files(apps, home)
        .into_iter()
        .map(|(path, readers)| json!({"path": path, "readers": readers, "exists": path.is_file()}))
        .collect();
    json!({ "files": files })
}

/// Open one listed, existing rules file in the system's text editor; a missing one is not created.
pub fn open(apps: &[App], home: &Path, path: &str) -> Result<Value, String> {
    let (path, _) = files(apps, home)
        .into_iter()
        .find(|(known, _)| known.as_os_str() == path)
        .ok_or("不在规则文件列表中")?;
    if !path.is_file() {
        return Err(format!("未找到 {}", path.display()));
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg("-t");
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("notepad.exe");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| format!("无法打开 {}", path.display()))?;
    Ok(json!({"ok": true, "message": format!("已打开 {}", path.display())}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_row_per_file_names_every_app_that_reads_it() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let app = |id, label, root: &str| App {
            id,
            label,
            root: h.join(root),
        };
        let apps = [
            app("claude", "Claude Code", ".claude"),
            app("grok", "Grok", ".grok"),
            app("codex", "Codex", ".codex"),
            app("antigravity", "Antigravity", ".gemini/config"),
            app("pi", "Pi", ".pi/agent"),
            app("zcode", "ZCode", ".zcode"),
        ];
        for file in [
            ".claude/CLAUDE.md",
            ".codex/AGENTS.override.md",
            ".codex/AGENTS.md",
            ".gemini/AGENTS.md",
            ".pi/agent/CLAUDE.md",
        ] {
            std::fs::create_dir_all(h.join(file).parent().unwrap()).unwrap();
            std::fs::write(h.join(file), "rules").unwrap();
        }
        let listed = list(&apps, h);
        let rows = listed["files"].as_array().unwrap();
        let row = |file: &str| {
            rows.iter()
                .find(|row| row["path"] == json!(h.join(file)))
                .cloned()
        };
        assert_eq!(
            row(".claude/CLAUDE.md").unwrap()["readers"],
            json!(["Claude Code", "Grok"])
        );
        // Codex and Pi read only the first that exists.
        assert!(row(".codex/AGENTS.override.md").is_some());
        assert!(row(".codex/AGENTS.md").is_none());
        assert_eq!(
            row(".pi/agent/CLAUDE.md").unwrap()["readers"],
            json!(["Pi"])
        );
        assert_eq!(
            row(".gemini/AGENTS.md").unwrap()["readers"],
            json!(["Antigravity"])
        );
        assert!(row(".gemini/GEMINI.md").is_none());
        // Grok already reads Claude's file; ZCode has none yet, so its usual one comes last.
        assert!(row(".grok/AGENTS.md").is_none());
        assert_eq!(
            rows.last().unwrap()["path"],
            json!(h.join(".zcode/AGENTS.md"))
        );
        assert_eq!(rows.last().unwrap()["exists"], false);

        assert!(
            open(&apps, h, h.join(".zcode/AGENTS.md").to_str().unwrap())
                .unwrap_err()
                .contains("未找到")
        );
        assert!(open(&apps, h, "/etc/hosts").is_err());
        assert!(!h.join(".zcode").exists()); // never created

        // Grok can be told not to read `~/.claude`; its own file counts under its exact name.
        std::fs::create_dir_all(h.join(".grok")).unwrap();
        std::fs::write(
            h.join(".grok/config.toml"),
            "[compat.claude]\nagents = false\n",
        )
        .unwrap();
        std::fs::write(h.join(".grok/Agents.md"), "rules").unwrap();
        let listed = list(&apps, h);
        let readers = |file: &str| {
            let rows = listed["files"].as_array().unwrap();
            let row = rows.iter().find(|row| row["path"] == json!(h.join(file)));
            row.map(|row| row["readers"].clone())
        };
        assert_eq!(readers(".claude/CLAUDE.md"), Some(json!(["Claude Code"])));
        assert_eq!(readers(".grok/Agents.md"), Some(json!(["Grok"])));
        assert_eq!(readers(".grok/AGENTS.md"), None);
    }
}
