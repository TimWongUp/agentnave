//! Where hosts keep their global rules. The App lists and opens them; it never writes them.
use serde_json::{Value, json};
use std::path::Path;

/// Common global-rule files under the home directory; each CLI's documentation is authoritative.
const RULES: [(&str, &str, &str); 6] = [
    ("claude", "Claude Code", ".claude/CLAUDE.md"),
    ("codex", "Codex CLI", ".codex/AGENTS.md"),
    ("codebuddy", "CodeBuddy Code", ".codebuddy/CODEBUDDY.md"),
    ("gemini", "Antigravity / Gemini CLI", ".gemini/GEMINI.md"),
    ("grok", "Grok CLI", ".grok/AGENTS.md"),
    ("pi", "Pi Coding Agent", ".pi/agent/AGENTS.md"),
];

pub fn list(home: &Path) -> Value {
    let files: Vec<Value> = RULES
        .iter()
        .map(|(id, label, relative)| {
            let path = home.join(relative);
            json!({"id": id, "label": label, "path": path, "exists": path.is_file()})
        })
        .collect();
    json!({ "files": files })
}

/// Open an existing rules file in the system's text editor; a missing one is not created.
pub fn open(home: &Path, id: &str) -> Result<Value, String> {
    let (_, label, relative) = RULES
        .iter()
        .find(|(key, ..)| *key == id)
        .ok_or("未知的规则文件")?;
    let path = home.join(relative);
    if !path.is_file() {
        return Err(format!("未找到 {label} 的全局规则文件：{}", path.display()));
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
    Ok(json!({"ok": true, "message": format!("已打开 {label} 的全局规则文件")}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_known_files_and_opens_only_existing_ones() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::fs::write(home.path().join(".codex/AGENTS.md"), "rules").unwrap();
        let listed = list(home.path());
        let files = listed["files"].as_array().unwrap();
        let codex = files.iter().find(|file| file["id"] == "codex").unwrap();
        assert_eq!(codex["exists"], true);
        assert!(
            files
                .iter()
                .filter(|file| file["id"] != "codex")
                .all(|file| file["exists"] == false)
        );
        assert!(open(home.path(), "claude").unwrap_err().contains("未找到"));
        assert!(open(home.path(), "../etc").is_err());
        assert!(!home.path().join(".claude").exists()); // never created
    }
}
