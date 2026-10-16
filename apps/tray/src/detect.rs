//! Which provider CLIs are installed here and which models they list (ADR-0014). Probes only run
//! each CLI's own listing or help command with empty stdin; they never start a model turn.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::members::PROVIDERS;

const TIMEOUT: Duration = Duration::from_secs(15);
/// The same common install locations the Python Adapters search after PATH.
const COMMON_DIRS: [&str; 8] = [
    "~/.local/bin",
    "~/.grok/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "~/.bun/bin",
    "~/.npm-global/bin",
    "~/.volta/bin",
    "~/.cargo/bin",
];
#[cfg(target_os = "macos")]
const DESKTOP_CODEX: [&str; 4] = [
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
    "~/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
    "/Applications/Codex.app/Contents/Resources/codex",
    "~/Applications/Codex.app/Contents/Resources/codex",
];
/// Known effort values, checked against each CLI's `--help` (Grok: its bundled `/effort` docs;
/// Codex: the model cache when present).
const EFFORTS: [(&str, &[&str]); 6] = [
    ("antigravity", &["low", "medium", "high", "xhigh", "max"]),
    ("claude", &["low", "medium", "high", "xhigh", "max"]),
    (
        "codebuddy",
        &["minimal", "low", "medium", "high", "xhigh", "max"],
    ),
    ("codex", &["minimal", "low", "medium", "high", "xhigh"]),
    ("grok", &["low", "medium", "high", "xhigh"]),
    (
        "pi",
        &["off", "minimal", "low", "medium", "high", "xhigh", "max"],
    ),
];
const EFFORT_ORDER: [&str; 8] = [
    "off", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];
const CLAUDE_ALIASES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

static CACHE: Mutex<Option<Value>> = Mutex::new(None);

/// Probe results for all providers, kept until `refresh` asks for a new probe.
pub fn detected(refresh: bool) -> Value {
    let mut cache = CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
    if refresh || cache.is_none() {
        let results: Vec<(&str, Value)> = thread::scope(|scope| {
            let probes: Vec<_> = PROVIDERS
                .iter()
                .map(|provider| (*provider, scope.spawn(|| probe(provider))))
                .collect();
            probes
                .into_iter()
                .map(|(provider, probe)| {
                    let failed = || failure(provider, None, "探测异常结束");
                    (provider, probe.join().unwrap_or_else(|_| failed()))
                })
                .collect()
        });
        let mut all = Map::new();
        for (provider, result) in results {
            all.insert(provider.to_owned(), result);
        }
        *cache = Some(Value::Object(all));
    }
    cache.clone().unwrap_or(Value::Null)
}

fn efforts(provider: &str) -> Vec<String> {
    EFFORTS
        .iter()
        .find(|(name, _)| *name == provider)
        .map(|(_, efforts)| efforts.iter().map(|e| (*e).to_owned()).collect())
        .unwrap_or_default()
}

fn model(id: &str, label: &str, default: bool) -> Value {
    json!({"id": id, "label": label, "default": default})
}

fn failure(provider: &str, path: Option<&Path>, error: &str) -> Value {
    json!({
        "installed": path.is_some(),
        "path": path.map(|p| p.display().to_string()),
        "source": "none",
        "error": (!error.is_empty()).then_some(error),
        "models": [],
        "efforts": efforts(provider),
    })
}

fn probe(provider: &str) -> Value {
    let binary = if provider == "antigravity" {
        "agy"
    } else {
        provider
    };
    let Some(path) = find(binary) else {
        return failure(provider, None, "");
    };
    let mut efforts = efforts(provider);
    let listed = match provider {
        "codex" => codex_models().map(|(models, levels)| {
            if !levels.is_empty() {
                efforts = levels;
            }
            ("codex-cache", models)
        }),
        "claude" => Ok((
            "aliases",
            CLAUDE_ALIASES
                .iter()
                .map(|alias| model(alias, alias, false))
                .collect(),
        )),
        "grok" => run(&path, &["models"]).map(|out| ("command", grok_models(&out))),
        "antigravity" => run(&path, &["models"]).map(|out| ("command", agy_models(&out))),
        "pi" => run(&path, &["--list-models"]).map(|out| ("command", pi_models(&out))),
        "codebuddy" => run(&path, &["--help"]).map(|out| ("help", codebuddy_models(&out))),
        _ => Err("未知 CLI".to_owned()),
    };
    match listed {
        Ok((source, models)) => json!({
            "installed": true,
            "path": path.display().to_string(),
            "source": source,
            "error": Value::Null,
            "models": models,
            "efforts": efforts,
        }),
        Err(error) => failure(provider, Some(&path), &error),
    }
}

fn expand(entry: &str) -> Option<PathBuf> {
    match entry.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|home| home.join(rest)),
        None => Some(PathBuf::from(entry)),
    }
}

fn common_dirs() -> Vec<PathBuf> {
    if cfg!(windows) {
        return Vec::new();
    }
    COMMON_DIRS
        .iter()
        .filter_map(|entry| expand(entry))
        .filter(|dir| dir.is_dir())
        .collect()
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    path.is_file()
}

/// The Codex desktop bundles on macOS, then PATH, then the common install directories.
fn find(name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    if name == "codex"
        && let Some(path) = DESKTOP_CODEX
            .iter()
            .filter_map(|entry| expand(entry))
            .find(|path| executable(path))
    {
        return Some(path);
    }
    let names: Vec<String> = if cfg!(windows) {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|ext| format!("{name}.{ext}"))
            .collect()
    } else {
        vec![name.to_owned()]
    };
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&inherited)
        .chain(common_dirs())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| executable(path))
}

/// PATH for a probe: the CLI's own directory and the common directories before the inherited
/// PATH, so `#!/usr/bin/env node` launchers work from an app started by the Finder.
fn search_path(executable: &Path) -> Option<std::ffi::OsString> {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = executable
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    for dir in common_dirs()
        .into_iter()
        .chain(std::env::split_paths(&inherited))
    {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).ok()
}

/// Run a listing command with empty stdin; give up after the timeout.
fn run(executable: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = search_path(executable) {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let (sender, receiver) = mpsc::channel();
    for (stream, pipe) in [
        (
            0,
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
        (
            1,
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
    ] {
        let sender = sender.clone();
        thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut text);
            }
            let _ = sender.send((stream, String::from_utf8_lossy(&text).into_owned()));
        });
    }
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} 秒内未完成", TIMEOUT.as_secs()));
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    // A descendant holding the pipes open must not stall the settings page.
    let (mut stdout, mut stderr) = (String::new(), String::new());
    for _ in 0..2 {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_secs(1));
        match receiver.recv_timeout(wait) {
            Ok((0, text)) => stdout = text,
            Ok((_, text)) => stderr = text,
            Err(_) => break,
        }
    }
    if status.success() {
        return Ok(stdout);
    }
    let reason = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(|| format!("退出码 {status}"), str::to_owned);
    Err(reason.chars().take(200).collect())
}

/// Codex's own model cache: listed models by priority, and the effort levels they support.
fn codex_models() -> Result<(Vec<Value>, Vec<String>), String> {
    let home = match std::env::var_os("CODEX_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::home_dir().unwrap_or_default().join(".codex"),
    };
    let text = std::fs::read_to_string(home.join("models_cache.json"))
        .map_err(|error| format!("无法读取 Codex 模型缓存：{error}"))?;
    parse_codex_cache(&text)
}

fn parse_codex_cache(text: &str) -> Result<(Vec<Value>, Vec<String>), String> {
    let cache: Value =
        serde_json::from_str(text).map_err(|error| format!("Codex 模型缓存损坏：{error}"))?;
    let mut entries: Vec<&Value> = cache["models"]
        .as_array()
        .ok_or("Codex 模型缓存缺少 models")?
        .iter()
        .filter(|entry| entry["visibility"].as_str() != Some("hide"))
        .collect();
    entries.sort_by_key(|entry| entry["priority"].as_i64().unwrap_or(i64::MAX));
    let mut levels: Vec<String> = Vec::new();
    let mut models = Vec::new();
    for entry in entries {
        let Some(id) = entry["slug"].as_str().or(entry["id"].as_str()) else {
            continue;
        };
        let label = entry["display_name"].as_str().unwrap_or(id);
        let mut own: Vec<&str> = Vec::new();
        for level in entry["supported_reasoning_levels"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(effort) = level["effort"].as_str().or(level.as_str())
                && !own.contains(&effort)
            {
                own.push(effort);
                if !levels.iter().any(|known| known == effort) {
                    levels.push(effort.to_owned());
                }
            }
        }
        let mut listed = model(id, label, false);
        // Per-model efforts where the cache knows them; the provider list is the fallback.
        if !own.is_empty() {
            listed["efforts"] = json!(own);
        }
        models.push(listed);
    }
    let rank = |effort: &String| {
        EFFORT_ORDER
            .iter()
            .position(|known| known == effort)
            .unwrap_or(EFFORT_ORDER.len())
    };
    levels.sort_by_key(rank);
    Ok((models, levels))
}

/// `grok models`: the entries under "Available models:", `(default)` marking the default.
fn grok_models(text: &str) -> Vec<Value> {
    let mut models = Vec::new();
    let mut listing = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("Available models:") {
            listing = true;
            continue;
        }
        if !listing {
            continue;
        }
        let Some(entry) = line
            .strip_prefix('*')
            .or_else(|| line.strip_prefix('-'))
            .map(str::trim)
        else {
            if !models.is_empty() {
                break;
            }
            continue;
        };
        let (id, default) = match entry.strip_suffix("(default)") {
            Some(id) => (id.trim(), true),
            None => (entry, false),
        };
        if !id.is_empty() {
            models.push(model(id, id, default));
        }
    }
    models
}

/// `agy models`: one `id<TAB>label` per line; progress lines carry no tab.
fn agy_models(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(id, _)| !id.trim().is_empty())
        .map(|(id, label)| {
            let label = label.trim();
            model(
                id.trim(),
                if label.is_empty() { id.trim() } else { label },
                false,
            )
        })
        .collect()
}

/// `pi --list-models`: a table whose provider and model columns form `provider/model`.
fn pi_models(text: &str) -> Vec<Value> {
    let mut lines = text.lines().skip_while(|line| {
        let mut columns = line.split_whitespace();
        !(columns.next() == Some("provider") && columns.next() == Some("model"))
    });
    if lines.next().is_none() {
        return Vec::new();
    }
    lines
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            Some(format!("{}/{}", columns.next()?, columns.next()?))
        })
        .map(|id| model(&id, &id, false))
        .collect()
}

/// `codebuddy --help`: the list in "Currently supported: (…)" under `--model`.
fn codebuddy_models(text: &str) -> Vec<Value> {
    let Some(start) = text.find("Currently supported: (") else {
        return Vec::new();
    };
    let rest = &text[start + "Currently supported: (".len()..];
    let Some(end) = rest.find(')') else {
        return Vec::new();
    };
    rest[..end]
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| model(id, id, false))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(models: &[Value]) -> Vec<&str> {
        models.iter().map(|m| m["id"].as_str().unwrap()).collect()
    }

    #[test]
    fn codex_cache_lists_visible_models_by_priority_with_their_efforts() {
        let cache = json!({"models": [
            {"slug": "gpt-6-luna", "display_name": "GPT-6-Luna", "visibility": "list", "priority": 4,
             "supported_reasoning_levels": [{"effort": "medium"}, {"effort": "max"}]},
            {"slug": "hidden", "visibility": "hide", "priority": 1,
             "supported_reasoning_levels": [{"effort": "secret"}]},
            {"id": "legacy", "priority": 2, "supported_reasoning_levels": ["low", "ultra"]},
            {"slug": "gpt-6.1-sol", "display_name": "GPT-6.1-Sol", "visibility": "list", "priority": 1}
        ]});
        let (models, efforts) = parse_codex_cache(&cache.to_string()).unwrap();
        assert_eq!(ids(&models), ["gpt-6.1-sol", "legacy", "gpt-6-luna"]);
        assert_eq!(models[0], model("gpt-6.1-sol", "GPT-6.1-Sol", false));
        assert_eq!(models[1]["efforts"], json!(["low", "ultra"]));
        assert_eq!(models[2]["efforts"], json!(["medium", "max"]));
        assert_eq!(efforts, ["low", "medium", "max", "ultra"]);
        assert!(parse_codex_cache("{").is_err());
        assert!(parse_codex_cache("{}").is_err());
    }

    #[test]
    fn command_listings_are_parsed_from_their_fixed_formats() {
        let grok = "You are logged in with grok.com.\n\nDefault model: grok-4.7\n\n\
            Available models:\n  * grok-4.7 (default)\n  - grok-4.7-build-fast\n  - grok-4.6\n\nTip\n";
        let models = grok_models(grok);
        assert_eq!(
            ids(&models),
            ["grok-4.7", "grok-4.7-build-fast", "grok-4.6"]
        );
        assert_eq!(models[0]["default"], true);
        assert_eq!(models[1]["default"], false);

        let agy = "Fetching available models...\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\n\
            claude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)\n";
        let models = agy_models(agy);
        assert_eq!(
            ids(&models),
            ["gemini-3.8-flash-high", "claude-opus-4-6-thinking"]
        );
        assert_eq!(models[0]["label"], "Gemini 3.8 Flash (High)");

        let pi = "provider     model                 context  max-out  thinking  images\n\
            magpie       codex/gpt-5.6-luna    272K     128K     yes       yes\n\
            opencode-go  glm-5.3               1M       131.1K   yes       no\n";
        assert_eq!(
            ids(&pi_models(pi)),
            ["magpie/codex/gpt-5.6-luna", "opencode-go/glm-5.3"]
        );
        assert!(pi_models("No models available\n").is_empty());

        let codebuddy = "  --model <model>   Model for the current session. Please provide the \
            model ID. Currently supported: (hy4-preview, glm-5.3, kimi-k2.7)\n  --effort <level>\n";
        assert_eq!(
            ids(&codebuddy_models(codebuddy)),
            ["hy4-preview", "glm-5.3", "kimi-k2.7"]
        );
        assert!(codebuddy_models("Usage: codebuddy").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn listing_commands_get_empty_stdin_a_completed_path_and_a_failure_reason() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("cli");
        std::fs::write(
            &script,
            "#!/bin/sh\nif [ \"$1\" = fail ]; then echo boom >&2; exit 3; fi\n\
             read line || echo empty-stdin\necho \"$PATH\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = run(&script, &[]).unwrap();
        let mut lines = out.lines();
        assert_eq!(lines.next(), Some("empty-stdin"));
        assert!(
            lines
                .next()
                .unwrap()
                .starts_with(&directory.path().display().to_string())
        );
        assert_eq!(run(&script, &["fail"]).unwrap_err(), "boom");
    }
}
