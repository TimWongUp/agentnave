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
const DESKTOP_DSH: [&str; 2] = [
    "/Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh",
    "~/Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh",
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
const EFFORTS: [(&str, &[&str]); 7] = [
    ("antigravity", &["low", "medium", "high", "xhigh", "max"]),
    ("claude", &["low", "medium", "high", "xhigh", "max"]),
    (
        "codebuddy",
        &["minimal", "low", "medium", "high", "xhigh", "max"],
    ),
    ("codex", &["minimal", "low", "medium", "high", "xhigh"]),
    ("deepseek", &["off", "low", "high", "max"]),
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
/// dsh has no listing command; its bundled DeepSeek catalog, shipped default first.
const DEEPSEEK_CATALOG: [&str; 2] = ["deepseek-flash", "deepseek-v4-pro"];

static CACHE: Mutex<Option<Value>> = Mutex::new(None);

/// Probe results for all providers, kept until `refresh` asks for a new probe. Probes run
/// outside the lock, so concurrent first reads may probe twice; the last result wins.
pub fn detected(refresh: bool) -> Value {
    let cache = || CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
    if !refresh && let Some(cached) = cache().clone() {
        return cached;
    }
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
    let all = Value::Object(all);
    *cache() = Some(all.clone());
    all
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
        "primary": Value::Null,
        "efforts": efforts(provider),
    })
}

/// The model the App pre-fills as a CLI's default: the listed model the CLI's own settings name
/// (`configured`, matched by id or label), else Codex's first-priority GPT model, Claude's
/// `opus` or the CLI's marked default, else the first listed model.
pub fn primary(provider: &str, models: &[Value], configured: Option<&str>) -> Option<String> {
    let id = |model: &Value| model["id"].as_str().map(str::to_owned);
    let named = configured.and_then(|name| {
        models
            .iter()
            .find(|model| model["id"] == name || model["label"] == name)
    });
    let preferred = match provider {
        "codex" => models
            .iter()
            .filter_map(id)
            .find(|model| model.starts_with("gpt-")),
        "claude" => models.iter().filter_map(id).find(|model| model == "opus"),
        _ => models
            .iter()
            .find(|model| model["default"] == true)
            .and_then(id),
    };
    named
        .and_then(id)
        .or(preferred)
        .or_else(|| models.first().and_then(id))
}

/// The default model a CLI's own settings file names, if any: Codex `config.toml` `model`,
/// Claude `settings.json` `model` without a `[1m]`-style suffix, Pi `defaultModel`, and the
/// Antigravity CLI's `model` label.
fn configured(provider: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let dir = |variable: &str, fallback: &str| match std::env::var_os(variable) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home.join(fallback),
    };
    let json = |path: PathBuf, key: &str| -> Option<String> {
        let text = std::fs::read_to_string(path).ok()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        value[key].as_str().map(str::to_owned)
    };
    let name = match provider {
        "codex" => {
            let text = std::fs::read_to_string(dir("CODEX_HOME", ".codex").join("config.toml"));
            let table: toml::Table = text.ok()?.parse().ok()?;
            table.get("model")?.as_str().map(str::to_owned)
        }
        "claude" => json(
            dir("CLAUDE_CONFIG_DIR", ".claude").join("settings.json"),
            "model",
        )
        .map(|model| model.split('[').next().unwrap_or_default().to_owned()),
        "pi" => {
            let settings = dir("PI_CODING_AGENT_DIR", ".pi/agent").join("settings.json");
            let model = json(settings.clone(), "defaultModel")?;
            // Pi lists `provider/model`; its settings may keep the provider apart.
            match json(settings, "defaultProvider") {
                Some(owner) if !model.starts_with(&format!("{owner}/")) => {
                    Some(format!("{owner}/{model}"))
                }
                _ => Some(model),
            }
        }
        "antigravity" => json(home.join(".gemini/antigravity-cli/settings.json"), "model"),
        _ => None,
    }?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn probe(provider: &str) -> Value {
    let binary = match provider {
        "antigravity" => "agy",
        "deepseek" => "dsh",
        other => other,
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
        "deepseek" => Ok((
            "catalog",
            DEEPSEEK_CATALOG
                .iter()
                .enumerate()
                .map(|(index, id)| model(id, id, index == 0))
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
            "primary": primary(provider, &models, configured(provider).as_deref()),
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

/// The Codex or DeepSeek Harness desktop bundles on macOS, then PATH, then the common
/// install directories.
fn find(name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let desktop: &[&str] = match name {
        "codex" => &DESKTOP_CODEX,
        "dsh" => &DESKTOP_DSH,
        _ => &[],
    };
    #[cfg(target_os = "macos")]
    if let Some(path) = desktop
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

/// PATH for a probe: the CLI's own directory, then the inherited PATH, then the common
/// directories, so `#!/usr/bin/env node` launchers work from an app started by the Finder
/// without overriding the toolchain the inherited PATH selects.
fn search_path(executable: &Path) -> Option<std::ffi::OsString> {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = executable
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    for dir in std::env::split_paths(&inherited).chain(common_dirs()) {
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
    // Its own process group, so a timeout also stops descendants holding the pipes.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
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
                kill_group(&mut child);
                let _ = child.wait();
                return Err(format!("{} 秒内未完成", TIMEOUT.as_secs()));
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    // A descendant still holding the pipes a second after the CLI exited is stopped with its
    // group; the group outlives its reaped leader while members remain, so its ID stays reserved.
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let mut received = 0;
    let mut killed = false;
    while received < 2 {
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok((0, text)) => stdout = text,
            Ok((_, text)) => stderr = text,
            Err(_) if !killed => {
                kill_group(&mut child);
                killed = true;
                continue;
            }
            Err(_) => break,
        }
        received += 1;
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

#[cfg(unix)]
fn kill_group(child: &mut std::process::Child) {
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: kill(2) only sends a signal; a negative ID names the probe's process group.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(child: &mut std::process::Child) {
    let _ = child.kill();
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
    // Help text may wrap at any space, including inside the marker.
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
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
        let wrapped = "  --model <model>   Please provide the model ID. Currently\n      \
            supported: (hy4-preview,\n      glm-5.3)\n";
        assert_eq!(ids(&codebuddy_models(wrapped)), ["hy4-preview", "glm-5.3"]);

        // The pre-filled default: the model the CLI's settings name, else its main model, else
        // the first listed one.
        let listed =
            |ids: &[&str]| -> Vec<Value> { ids.iter().map(|id| model(id, id, false)).collect() };
        let codex = listed(&["opencode-go/glm-5.3-flash", "gpt-6.1-sol", "gpt-6-luna"]);
        assert_eq!(
            primary("codex", &codex, None).as_deref(),
            Some("gpt-6.1-sol")
        );
        assert_eq!(
            primary("codex", &codex, Some("gpt-6-luna")).as_deref(),
            Some("gpt-6-luna")
        );
        let claude = listed(&CLAUDE_ALIASES);
        assert_eq!(primary("claude", &claude, None).as_deref(), Some("opus"));
        assert_eq!(
            primary("grok", &grok_models(grok), None).as_deref(),
            Some("grok-4.7")
        );
        let agy = agy_models(agy);
        assert_eq!(
            primary("antigravity", &agy, None).as_deref(),
            Some("gemini-3.8-flash-high")
        );
        let label = agy[1]["label"].as_str().unwrap();
        assert_eq!(
            primary("antigravity", &agy, Some(label)),
            agy[1]["id"].as_str().map(str::to_owned)
        );
        let pi = pi_models(pi);
        assert_eq!(
            primary("pi", &pi, None),
            pi[0]["id"].as_str().map(str::to_owned)
        );
        assert_eq!(
            primary("codebuddy", &codebuddy_models(codebuddy), Some("unlisted")).as_deref(),
            Some("hy4-preview")
        );
        assert_eq!(
            primary("codex", &listed(&["grok/grok-4.7"]), None).as_deref(),
            Some("grok/grok-4.7")
        );
        assert_eq!(primary("pi", &[], Some("anything")), None);
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
        // The CLI's own directory, then the inherited PATH ahead of the common directories.
        let path: Vec<String> = lines
            .next()
            .unwrap()
            .split(':')
            .map(str::to_owned)
            .collect();
        assert_eq!(path[0], directory.path().display().to_string());
        let inherited = std::env::var("PATH").unwrap_or_default();
        if let Some(first) = inherited
            .split(':')
            .find(|dir| *dir != path[0] && !dir.is_empty())
        {
            assert_eq!(path[1], first);
        }
        assert_eq!(run(&script, &["fail"]).unwrap_err(), "boom");

        // A grandchild keeping the pipe open after the CLI exited is stopped with its group.
        let lingering = directory.path().join("lingering");
        std::fs::write(
            &lingering,
            "#!/bin/sh
echo listed
sleep 30 &
",
        )
        .unwrap();
        std::fs::set_permissions(&lingering, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();
        assert_eq!(run(&lingering, &[]).unwrap(), "listed\n");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
