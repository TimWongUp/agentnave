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

/// The mode a boss row is set to: its own, or the default that rows without one follow.
fn mode_of<'a>(document: &'a Value, key: &str) -> &'a str {
    document["apps"][key]
        .as_str()
        .or(document["preset"].as_str())
        .unwrap_or("off")
}

/// The text a mode puts in the rules files, if any.
fn text_of(document: &Value, mode: &str) -> Option<String> {
    match mode {
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
    /// The boss row, which has its own mode; rows of one app share `id`.
    pub key: &'static str,
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
fn targets(connected: &[&App], home: &Path) -> Vec<PathBuf> {
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

/// Connected apps that read one another's files, in `connected` order. One block reaches them all,
/// so each group has one mode: that of its first app.
fn groups<'a>(connected: &'a [App], home: &Path) -> Vec<Vec<&'a App>> {
    let mut groups: Vec<(Vec<usize>, Vec<PathBuf>)> = Vec::new();
    for (index, app) in connected.iter().enumerate() {
        let mut merged = (vec![index], would_read(app, home));
        merged.1.extend(reads(app, home));
        groups.retain(|(members, paths)| {
            let apart = !paths.iter().any(|path| merged.1.contains(path));
            if !apart {
                merged.0.extend(members);
                merged.1.extend(paths.iter().cloned());
            }
            apart
        });
        groups.push(merged);
    }
    groups.sort_by_key(|(members, _)| members.iter().min().copied());
    groups
        .into_iter()
        .map(|(mut members, _)| {
            members.sort_unstable();
            members.into_iter().map(|index| &connected[index]).collect()
        })
        .collect()
}

/// The text each file that carries the block should hold.
fn wanted(connected: &[App], home: &Path, document: &Value) -> Vec<(PathBuf, String)> {
    let mut wanted = Vec::new();
    for group in groups(connected, home) {
        if let Some(text) = text_of(document, mode_of(document, group[0].key)) {
            wanted.extend(
                targets(&group, home)
                    .into_iter()
                    .map(|path| (path, text.clone())),
            );
        }
    }
    wanted
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

/// Put each boss's mode in the block of its target files and take the block out of every other
/// rules file an installed app could read, and of `also` (files read before a host moved).
/// Returns the files that could not be written.
pub fn sync(
    installed: &[App],
    connected: &[App],
    home: &Path,
    document: &Value,
    also: &[PathBuf],
    backup: &Backup<'_>,
) -> Vec<Value> {
    let wanted = wanted(connected, home, document);
    let mut paths: Vec<PathBuf> = wanted.iter().map(|(path, _)| path.clone()).collect();
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
            let text = wanted.iter().find(|(known, _)| known == path);
            write(path, home, text.map(|(_, text)| text.as_str()), backup)
                .err()
                .map(|error| json!({"path": path, "error": error}))
        })
        .collect()
}

pub fn list(installed: &[App], connected: &[App], home: &Path, document: &Value) -> Value {
    let wanted = wanted(connected, home, document);
    let files: Vec<Value> = files(installed, home)
        .into_iter()
        .map(|(path, readers)| {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let text = wanted.iter().find(|(known, _)| *known == path);
            let current = match (find(&content), text) {
                (Ok(Some((start, end))), Some((_, text))) => content[start..end] == block(text),
                _ => false,
            };
            json!({"path": path, "readers": readers, "exists": path.is_file(),
                "target": text.is_some(), "current": current,
                "blocked": crate::hosts::check_path(&path, home).is_err()})
        })
        .collect();
    let bosses: Vec<Value> = groups(connected, home)
        .into_iter()
        .map(|group| {
            json!({"keys": group.iter().map(|app| app.key).collect::<Vec<_>>(),
                "labels": group.iter().map(|app| app.label).collect::<Vec<_>>(),
                "mark": group[0].id})
        })
        .collect();
    json!({ "files": files, "bosses": bosses })
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
        .stderr(std::process::Stdio::null());
    let failed = || format!("无法打开 {}", path.display());
    // `open` returns at once and says whether it worked; the others stay until the editor closes.
    #[cfg(target_os = "macos")]
    command
        .status()
        .ok()
        .filter(std::process::ExitStatus::success)
        .ok_or_else(failed)?;
    #[cfg(not(target_os = "macos"))]
    command.spawn().map_err(|_| failed())?;
    Ok(json!({"ok": true, "message": format!("已打开 {}", path.display())}))
}

#[cfg(test)]
mod tests;
