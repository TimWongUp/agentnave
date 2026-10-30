//! Where the boss apps keep their global rules. The App lists and opens them, and keeps its
//! orchestration preset in one marked block of them; everything outside the block stays the user's.
//! One file often serves several apps (Grok also reads Claude's, dsh the shared `~/.agents`), so
//! the list is by file, each naming every app that reads it.
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const PRESETS: [&str; 4] = ["off", "split", "parallel", "custom"];
const START: &str = "<!-- AGENTNAVE:START -->";
const END: &str = "<!-- AGENTNAVE:END -->";
const CUSTOM_LIMIT: usize = 8000;

const SHARED: &str = "## AgentNave 编排偏好

- 仅在作为主对话 boss 时适用；被 AgentNave 派发时只完成任务、回报，不再派发。与本文件其他规则冲突时，以其他规则为准。
- 本偏好不新增推送、合并 PR、发布、发送的授权，也不追加权限参数；派发前按 agentnave-manager 调用。
- Explorer（只读：探索、规划、评审）与 Worker（可写：实现）各有简单、困难两档，按任务难度二选一，取 `list_members` 的 `orchestration`：标为 `native` 的交给宿主自带子代理并尽量用该模型，其余经 AgentNave 派发，`role` 用 `explorer-easy`、`explorer-hard`、`worker-easy` 或 `worker-hard`，服务套用 App 里选的模型；没有设置时按任务组员选，都不可用时自己做。
- 选择本档即授权在已确认的任务内主动派发，包括实现，无需用户点名 CLI；小事自己做。
- 探索、规划、评审派 Explorer，prompt 写明不改文件；实现派 Worker。评审不交给原来的 Worker，尽量换模型。
- 同一组文件同一时间只有一个 Worker 在写；同一职责可以续接，换职责时给自足的简报，不让下一位重新探索。
- 评审按风险定级；启用 agentnave-review 时，评审的定级和选工位都按它，不用 Explorer。
- `succeeded` 不等于完成，boss 核对产物后验收；失败或取消时先核对现状，再决定续接、重派还是收尾。";

const PARALLEL: &str = "
- 计划确认后，互不依赖的写入步骤并行派给多个 Worker，在 Git 仓库里各用独立 worktree；无法隔离时串行。boss 合入当前分支，最后统一验收。
- 有实质取舍或分歧时进会议室出决定，会上不改代码。
- 非琐碎的实现换一个 Explorer 做只读复核；高风险改动按具体的失败假设追加验证。
- 没有新证据或待解问题时，不再追加讨论、重试和复核。";

/// Each preset's rules text, for the settings page and the custom editor's starting text.
pub fn presets() -> Value {
    json!({"split": SHARED, "parallel": format!("{SHARED}{PARALLEL}")})
}

/// The text the saved orchestration settings put in the rules files, if any.
pub fn chosen(document: &Value) -> Option<String> {
    match document["preset"].as_str()? {
        "split" => Some(SHARED.into()),
        "parallel" => Some(format!("{SHARED}{PARALLEL}")),
        "custom" => Some(document["custom"].as_str()?.trim())
            .filter(|text| !text.is_empty())
            .map(Into::into),
        _ => None,
    }
}

/// Custom text goes into every boss app's rules each turn, so it stays short and cannot end the
/// block early.
pub fn check_custom(text: &str) -> Result<(), String> {
    if text.chars().count() > CUSTOM_LIMIT {
        return Err(format!("自定义编排最多 {CUSTOM_LIMIT} 字"));
    }
    if text.contains("<!-- AGENTNAVE:") {
        return Err("自定义编排不能包含 AgentNave 的标记".into());
    }
    Ok(())
}

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

/// The file a name opens, under the name it has on disk: on a case-insensitive disk `Agents.md`
/// and `AGENTS.md` are one file, shown once as it is spelled.
fn on_disk(path: &Path) -> Option<PathBuf> {
    if !path.is_file() {
        return None;
    }
    let (dir, name) = (path.parent()?, path.file_name()?.to_string_lossy());
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    let mut names: Vec<_> = entries.map(|entry| entry.file_name()).collect();
    names.sort_by_key(|own| own.to_string_lossy() != name);
    let own = names
        .into_iter()
        .find(|own| own.to_string_lossy().eq_ignore_ascii_case(&name));
    Some(own.map_or_else(|| path.to_owned(), |own| dir.join(own)))
}

/// The files an app actually reads, or its usual file when it has none yet.
fn reads(app: &App, home: &Path) -> Vec<PathBuf> {
    let (first, candidates) = candidates(app, home);
    let mut existing: Vec<PathBuf> = Vec::new();
    for path in candidates.iter().filter_map(|path| on_disk(path)) {
        if !existing.contains(&path) {
            existing.push(path);
        }
    }
    match (first, existing.is_empty()) {
        (_, true) => candidates
            .iter()
            .find(|p| p.ends_with("AGENTS.md"))
            .or(candidates.first())
            .into_iter()
            .cloned()
            .collect(),
        (true, false) => existing.into_iter().take(1).collect(),
        (false, false) => existing,
    }
}

/// Each file an app actually reads, or its usual file when it has none yet.
fn files(apps: &[App], home: &Path) -> Vec<(PathBuf, Vec<&'static str>)> {
    let mut out: Vec<(PathBuf, Vec<&'static str>)> = Vec::new();
    for app in apps {
        for path in reads(app, home) {
            match out.iter_mut().find(|(known, _)| *known == path) {
                Some((_, readers)) => readers.push(app.label),
                None => out.push((path, vec![app.label])),
            }
        }
    }
    // Files that exist first, then the ones not created yet.
    out.sort_by_key(|(path, _)| !path.is_file());
    out
}

/// Every file an app would read once it exists: an app that reads all it finds also reads a file
/// written for another app.
fn would_read(app: &App, home: &Path) -> Vec<PathBuf> {
    let (first, candidates) = candidates(app, home);
    if first {
        return reads(app, home);
    }
    let candidates = candidates.iter();
    candidates
        .map(|path| on_disk(path).unwrap_or_else(|| path.clone()))
        .collect()
}

/// The files that carry the block: one per connected app, so each reads it once. Apps that can
/// read fewer files choose first, and a chosen file covers every app that would read it.
fn targets(connected: &[App], home: &Path) -> Vec<PathBuf> {
    let mut apps: Vec<_> = connected
        .iter()
        .map(|app| (reads(app, home), would_read(app, home)))
        .collect();
    apps.sort_by_key(|(_, covered)| covered.len());
    let mut chosen: Vec<PathBuf> = Vec::new();
    for (read, covered) in apps {
        if !covered.iter().any(|path| chosen.contains(path))
            && let Some(path) = read.into_iter().next()
        {
            chosen.push(path);
        }
    }
    chosen
}

fn block(text: &str) -> String {
    format!("{START}\n{text}\n{END}")
}

/// Where the block sits: from its start marker to the end of its end marker. A lone or repeated
/// marker is refused: taking whatever lies between them could swallow the user's own rules.
fn find(content: &str) -> Result<Option<(usize, usize)>, String> {
    let broken = || "AgentNave 标记不完整或重复，请打开文件手动修复".to_owned();
    match (content.matches(START).count(), content.matches(END).count()) {
        (0, 0) => Ok(None),
        (1, 1) => {
            let (start, end) = (content.find(START).unwrap(), content.find(END).unwrap());
            (start < end)
                .then_some(Some((start, end + END.len())))
                .ok_or_else(broken)
        }
        _ => Err(broken()),
    }
}

/// The content with the block replaced in place, added at the end, or removed with the blank
/// lines around it.
fn splice(content: &str, text: Option<&str>) -> Result<String, String> {
    Ok(match (find(content)?, text) {
        (Some((start, end)), Some(text)) => {
            format!("{}{}{}", &content[..start], block(text), &content[end..])
        }
        // Undo what adding it put around it: the newline after it and, at the end of the file, the
        // blank line before it. The user's own spacing stays.
        (Some((start, end)), None) => {
            // An editor may have saved the file with CRLF since.
            let newline = |s: &str| ["\r\n", "\n"].into_iter().find(|n| s.ends_with(n));
            let rest = &content[end..];
            let after = ["\r\n", "\n"]
                .into_iter()
                .find_map(|n| rest.strip_prefix(n))
                .unwrap_or(rest);
            let before = &content[..start];
            let before = match newline(before).map(|n| &before[..before.len() - n.len()]) {
                Some(trimmed) if after.is_empty() && newline(trimmed).is_some() => trimmed,
                _ => before,
            };
            format!("{before}{after}")
        }
        (None, Some(text)) => {
            // One blank line before it, which removing it takes away again.
            let gap = match content {
                "" => "",
                _ if content.ends_with('\n') => "\n",
                _ => "\n\n",
            };
            format!("{content}{gap}{}\n", block(text))
        }
        (None, None) => content.into(),
    })
}

/// Saves a copy of a rules file's bytes before it is rewritten.
pub type Backup<'a> = dyn Fn(&Path, &[u8]) -> Result<(), String> + 'a;

/// Write one rules file, backing up what it held. Like the host config writes this is optimistic:
/// a change by another app since reading makes it give up rather than overwrite.
fn write(path: &Path, home: &Path, text: Option<&str>, backup: &Backup) -> Result<bool, String> {
    crate::hosts::check_path(path, home)
        .map_err(|error| crate::hosts::message(&error).to_owned())?;
    let original = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("无法读取".into()),
    };
    let content = match &original {
        Some(bytes) => std::str::from_utf8(bytes).map_err(|_| "不是 UTF-8 文本")?,
        None if text.is_none() => return Ok(false),
        None => "",
    };
    let next = splice(content, text)?;
    if next == content && original.is_some() {
        return Ok(false);
    }
    if let Some(bytes) = &original {
        backup(path, bytes)?;
    }
    let reread = |expected: &Option<Vec<u8>>| match std::fs::read(path) {
        Ok(bytes) => Some(bytes) == *expected,
        Err(error) => error.kind() == std::io::ErrorKind::NotFound && expected.is_none(),
    };
    // Only the block was ours: a file left empty without it goes away.
    if next.is_empty() {
        if !reread(&original) {
            return Err("文件刚被其他程序改动，未写入".into());
        }
        std::fs::remove_file(path).map_err(|_| "无法删除")?;
        return Ok(true);
    }
    let parent = path.parent().ok_or("路径无父目录")?;
    std::fs::create_dir_all(parent).map_err(|_| "无法创建目录")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法暂存")?;
    if let Ok(meta) = std::fs::metadata(path) {
        tmp.as_file()
            .set_permissions(meta.permissions())
            .map_err(|_| "无法保留文件权限")?;
    }
    tmp.write_all(next.as_bytes())
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|_| "无法暂存")?;
    if !reread(&original) {
        return Err("文件刚被其他程序改动，未写入".into());
    }
    // A folder swapped for a link since the first check would send the rename elsewhere.
    crate::hosts::check_path(path, home)
        .map_err(|error| crate::hosts::message(&error).to_owned())?;
    tmp.persist(path).map_err(|_| "无法替换文件")?;
    Ok(true)
}

/// Every existing rules file an installed app could read, including one it stopped reading.
pub fn existing(installed: &[App], home: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for app in installed {
        for path in candidates(app, home)
            .1
            .iter()
            .filter_map(|path| on_disk(path))
        {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

/// Put `text` in the block of every target file and take it out of every other rules file an
/// installed app could read, and of `also` (files read before a host moved); with no text, take
/// it out everywhere. Returns the files that could not be written.
pub fn sync(
    installed: &[App],
    connected: &[App],
    home: &Path,
    text: Option<&str>,
    also: &[PathBuf],
    backup: &Backup<'_>,
) -> Vec<Value> {
    let targets = if text.is_some() {
        targets(connected, home)
    } else {
        Vec::new()
    };
    let mut paths = targets.clone();
    for path in existing(installed, home)
        .into_iter()
        .chain(also.iter().cloned())
    {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
        .iter()
        .filter_map(|path| {
            let wanted = text.filter(|_| targets.contains(path));
            write(path, home, wanted, backup)
                .err()
                .map(|error| json!({"path": path, "error": error}))
        })
        .collect()
}

pub fn list(installed: &[App], connected: &[App], home: &Path, text: Option<&str>) -> Value {
    let targets = if text.is_some() {
        targets(connected, home)
    } else {
        Vec::new()
    };
    let files: Vec<Value> = files(installed, home)
        .into_iter()
        .map(|(path, readers)| {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let current = match (find(&content), text) {
                (Ok(Some((start, end))), Some(text)) => content[start..end] == block(text),
                _ => false,
            };
            json!({"path": path, "readers": readers, "exists": path.is_file(),
                "target": targets.contains(&path), "current": current,
                "blocked": crate::hosts::check_path(&path, home).is_err()})
        })
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
        let listed = list(&apps, &[], h, None);
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
        let listed = list(&apps, &[], h, None);
        let readers = |file: &str| {
            let rows = listed["files"].as_array().unwrap();
            let row = rows.iter().find(|row| row["path"] == json!(h.join(file)));
            row.map(|row| row["readers"].clone())
        };
        assert_eq!(readers(".claude/CLAUDE.md"), Some(json!(["Claude Code"])));
        assert_eq!(readers(".grok/Agents.md"), Some(json!(["Grok"])));
        assert_eq!(readers(".grok/AGENTS.md"), None);
    }

    #[test]
    fn the_block_goes_once_into_each_connected_app_and_leaves_the_rest_untouched() {
        let home = tempfile::tempdir().unwrap();
        let base = home.path().canonicalize().unwrap();
        let h = base.as_path();
        let app = |id, label, root: &str| App {
            id,
            label,
            root: h.join(root),
        };
        let claude = app("claude", "Claude Code", ".claude");
        let grok = app("grok", "Grok", ".grok");
        let codex = app("codex", "Codex", ".codex");
        let installed = [
            app("claude", "Claude Code", ".claude"),
            app("grok", "Grok", ".grok"),
            app("codex", "Codex", ".codex"),
        ];
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        std::fs::create_dir_all(h.join(".grok")).unwrap();
        let own = "# 我的规则\n\n- 保留\n";
        std::fs::write(h.join(".claude/CLAUDE.md"), own).unwrap();
        // A block left in a file no connected app needs any more is taken out.
        std::fs::write(
            h.join(".grok/AGENTS.md"),
            format!("grok\n\n{}\n", block("旧")),
        )
        .unwrap();
        let backups = std::cell::RefCell::new(Vec::new());
        let backup = |path: &Path, _: &[u8]| {
            backups.borrow_mut().push(path.to_owned());
            Ok(())
        };
        let connected = [claude, grok, codex];
        let text = chosen(&json!({"preset": "split"})).unwrap();
        assert!(sync(&installed, &connected, h, Some(&text), &[], &backup).is_empty());
        let claude_md = std::fs::read_to_string(h.join(".claude/CLAUDE.md")).unwrap();
        assert_eq!(
            claude_md,
            format!("# 我的规则\n\n- 保留\n\n{}\n", block(&text))
        );
        // Grok reads Claude's file too, so its own carries no second copy.
        assert_eq!(
            std::fs::read_to_string(h.join(".grok/AGENTS.md")).unwrap(),
            "grok\n"
        );
        assert!(
            std::fs::read_to_string(h.join(".codex/AGENTS.md"))
                .unwrap()
                .contains(START)
        );
        let listed = list(&installed, &connected, h, Some(&text));
        let current: Vec<_> = listed["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["current"] == true)
            .map(|row| row["path"].clone())
            .collect();
        assert_eq!(
            current,
            [
                json!(h.join(".claude/CLAUDE.md")),
                json!(h.join(".codex/AGENTS.md"))
            ]
        );

        // Unchanged settings write nothing; turning it off restores every file as it was.
        backups.borrow_mut().clear();
        assert!(sync(&installed, &connected, h, Some(&text), &[], &backup).is_empty());
        assert!(backups.borrow().is_empty());
        assert!(sync(&installed, &connected, h, None, &[], &backup).is_empty());
        assert_eq!(
            std::fs::read_to_string(h.join(".claude/CLAUDE.md")).unwrap(),
            own
        );
        assert!(!h.join(".codex/AGENTS.md").exists());

        // Neither file exists yet: Grok would read the one created for Claude, so it gets no other.
        std::fs::remove_file(h.join(".claude/CLAUDE.md")).unwrap();
        std::fs::remove_file(h.join(".grok/AGENTS.md")).unwrap();
        assert!(sync(&installed, &connected, h, Some(&text), &[], &backup).is_empty());
        assert!(h.join(".claude/CLAUDE.md").is_file());
        assert!(!h.join(".grok/AGENTS.md").exists());
        // A lone marker is left for the user rather than guessed at.
        let broken = format!("mine\n{START}\nold\n");
        std::fs::write(h.join(".codex/AGENTS.md"), &broken).unwrap();
        let failures = sync(&installed, &connected, h, Some(&text), &[], &backup);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0]["path"], json!(h.join(".codex/AGENTS.md")));
        assert_eq!(
            std::fs::read_to_string(h.join(".codex/AGENTS.md")).unwrap(),
            broken
        );

        // Adding and removing the block leaves the user's own spacing as it was.
        for own in ["我的规则\n\n\n", "我的规则\n", "我的规则\r\n"] {
            let added = splice(own, Some("x")).unwrap();
            assert_eq!(splice(&added, None).unwrap(), own);
        }
        let resaved = splice("我的规则\n", Some("x"))
            .unwrap()
            .replace('\n', "\r\n");
        assert_eq!(splice(&resaved, None).unwrap(), "我的规则\r\n");
        let alone = splice("", Some("x")).unwrap().replace('\n', "\r\n");
        assert_eq!(splice(&alone, None).unwrap(), "");
        // A file a moved host read before still loses its block.
        let moved = h.join("old-root/AGENTS.md");
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        std::fs::write(&moved, format!("old\n\n{}\n", block("旧"))).unwrap();
        sync(
            &installed,
            &connected,
            h,
            None,
            std::slice::from_ref(&moved),
            &backup,
        );
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "old\n");

        assert!(check_custom("<!-- AGENTNAVE:END -->").is_err());
        assert_eq!(chosen(&json!({"preset": "custom", "custom": "  "})), None);
    }
}
