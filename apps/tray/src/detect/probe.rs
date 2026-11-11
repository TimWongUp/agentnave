use super::*;
use crate::process::kill_group;

/// PATH for a probe: the CLI's own directory, then the inherited PATH, then the common
/// directories, so `#!/usr/bin/env node` launchers work from an app started by the Finder
/// without overriding the toolchain the inherited PATH selects.
pub(crate) fn search_path(executable: &Path) -> Option<std::ffi::OsString> {
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
pub(super) fn command(executable: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(std::env::temp_dir())
        .stdout(Stdio::piped());
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
    command
}

/// Send one request line to a CLI serving requests on stdin and return the first JSON line
/// `answers` accepts, then stop the CLI with its group. stdin stays open until then, since these
/// CLIs exit when it closes.
pub(super) fn ask(
    executable: &Path,
    args: &[&str],
    request: &Value,
    answers: impl Fn(&Value) -> bool,
) -> Result<Value, String> {
    let mut child = command(executable, args)
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdin = child.stdin.take();
    let written = stdin
        .map(|mut stdin| writeln!(stdin, "{request}").map(|()| stdin))
        .transpose();
    let (sender, receiver) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && sender.send(value).is_err()
                {
                    break;
                }
            }
        });
    }
    let deadline = Instant::now() + TIMEOUT;
    let answer = loop {
        if written.is_err() {
            break Err("无法发送请求".to_owned());
        }
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(value) if answers(&value) => break Ok(value),
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                break Err(format!("{} 秒内未完成", TIMEOUT.as_secs()));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break Err("没有回应".to_owned()),
        }
    };
    kill_group(&mut child);
    let _ = child.wait();
    answer
}

/// Claude Code's own model picker, from its `initialize` control response with hooks off: every
/// entry but `default`, each with the efforts it offers (none when the model has no effort).
pub(super) fn claude_models(path: &Path) -> Result<Vec<Value>, String> {
    let request = json!({"type": "control_request", "request_id": "agentnave-models",
        "request": {"subtype": "initialize"}});
    let args = [
        "-p",
        "--settings",
        r#"{"disableAllHooks":true}"#,
        "--no-session-persistence",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
    ];
    let response = ask(path, &args, &request, |line| {
        line["type"] == "control_response"
    })?;
    let models = claude_picker(&response);
    if models.is_empty() {
        return Err("Claude 没有列出模型".to_owned());
    }
    Ok(models)
}

pub(super) fn claude_picker(response: &Value) -> Vec<Value> {
    let entries = response["response"]["response"]["models"].as_array();
    entries
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let id = entry["value"].as_str().filter(|id| *id != "default")?;
            let mut listed = model(id, entry["displayName"].as_str().unwrap_or(id), false);
            listed["efforts"] = match &entry["supportedEffortLevels"] {
                Value::Array(levels) => Value::Array(levels.clone()),
                _ => json!([]),
            };
            // An alias such as `opus` keeps its per-model settings under the full ID.
            if let Some(resolved) = entry["resolvedModel"].as_str().filter(|r| *r != id) {
                listed["resolved"] = json!(resolved);
            }
            Some(listed)
        })
        .collect()
}

/// Pi's available models over RPC, as `provider/model` with the thinking levels Pi offers.
pub(super) fn pi_rpc_models(path: &Path) -> Result<Vec<Value>, String> {
    let request = json!({"id": "agentnave-models", "type": "get_available_models"});
    let response = ask(path, &["--mode", "rpc", "--no-session"], &request, |line| {
        line["id"] == "agentnave-models"
    })?;
    let models = response["data"]["models"]
        .as_array()
        .ok_or("Pi 没有列出模型")?;
    Ok(models
        .iter()
        .filter_map(|entry| {
            let id = format!("{}/{}", entry["provider"].as_str()?, entry["id"].as_str()?);
            let mut listed = model(&id, &id, false);
            listed["efforts"] = json!(pi_levels(entry));
            Some(listed)
        })
        .collect())
}

/// Pi's `getSupportedThinkingLevels`: only `off` without reasoning; otherwise every level the
/// model's map does not null out, `xhigh` and `max` only where the map names them.
pub(super) fn pi_levels(entry: &Value) -> Vec<&'static str> {
    if entry["reasoning"] != true {
        return vec!["off"];
    }
    let map = &entry["thinkingLevelMap"];
    ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
        .into_iter()
        .filter(|level| match map.get(level) {
            Some(Value::Null) => false,
            Some(_) => true,
            None => !matches!(*level, "xhigh" | "max"),
        })
        .collect()
}

/// The efforts Grok's model cache lists for one model, `None` when the cache does not know it.
pub(super) fn grok_efforts(cache: &Value, model: &str) -> Option<Vec<String>> {
    let info = &cache["models"].get(model)?["info"];
    let efforts = info["reasoning_efforts"].as_array();
    Some(
        efforts
            .into_iter()
            .flatten()
            .filter_map(|effort| effort["value"].as_str().map(str::to_owned))
            .collect(),
    )
}

/// The `reasoning` settings CodeBuddy's cached product configuration keeps per model
/// (`~/.codebuddy/local_storage/*.info`: a JSON string of base64 gzip JSON): `supportedEfforts`
/// and its default level, `defaultEffort` or `effort`.
pub(super) fn codebuddy_reasoning() -> Map<String, Value> {
    let Some(dir) = codebuddy_dir().map(|dir| dir.join("local_storage")) else {
        return Map::new();
    };
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    entries
        .filter_map(|entry| {
            let text: String =
                serde_json::from_str(&std::fs::read_to_string(entry.path()).ok()?).ok()?;
            let config = codebuddy_product_config(&text)?;
            (config["productName"].is_string()).then_some(config)
        })
        .flat_map(|config| {
            let models = config["models"].as_array().cloned().unwrap_or_default();
            models.into_iter().filter_map(|model| {
                let reasoning = model["reasoning"].clone();
                reasoning
                    .is_object()
                    .then_some((model["id"].as_str()?.to_owned(), reasoning))
            })
        })
        .collect()
}

pub(super) fn codebuddy_product_config(encoded: &str) -> Option<Value> {
    use base64::Engine;
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let mut text = String::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

pub(super) fn run(executable: &Path, args: &[&str]) -> Result<String, String> {
    let mut child = command(executable, args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
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
    let status = match crate::process::wait(
        &mut child,
        || Instant::now() < deadline,
        Duration::from_millis(50),
    ) {
        Ok(Some(status)) => status,
        Ok(None) => {
            crate::process::stop(&mut child, false);
            return Err(format!("{} 秒内未完成", TIMEOUT.as_secs()));
        }
        Err(error) => return Err(error.to_string()),
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

/// Codex's model list: the catalog its own config names, else the cache it keeps.
///
/// A gateway such as magpie rewrites `model_catalog_json` whenever its providers change, but
/// Codex refreshes `models_cache.json` on its own schedule, so the cache can keep models a
/// provider no longer offers and miss the ones it just added. The catalog wins when both exist.
pub(super) fn codex_models() -> Result<(Vec<Value>, Vec<String>), String> {
    codex_models_in(&codex_home())
}

fn codex_home() -> PathBuf {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::home_dir().unwrap_or_default().join(".codex"),
    }
}

pub(super) fn codex_models_in(home: &Path) -> Result<(Vec<Value>, Vec<String>), String> {
    if let Some(catalog) = codex_catalog(home)
        && let Ok(text) = std::fs::read_to_string(catalog)
    {
        return parse_codex_cache(&text);
    }
    let text = std::fs::read_to_string(home.join("models_cache.json"))
        .map_err(|error| format!("无法读取 Codex 模型缓存：{error}"))?;
    parse_codex_cache(&text)
}

/// The `model_catalog_json` file Codex's `config.toml` points at, resolved against its home the
/// way Codex does; a relative name is relative to `CODEX_HOME`, not the working directory.
fn codex_catalog(home: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(home.join("config.toml")).ok()?;
    let table: toml::Table = text.parse().ok()?;
    let named = table.get("model_catalog_json")?.as_str()?;
    let path = Path::new(named);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        home.join(path)
    })
}

pub(super) fn parse_codex_cache(text: &str) -> Result<(Vec<Value>, Vec<String>), String> {
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
        if entry["supported_reasoning_levels"].is_array() {
            listed["efforts"] = json!(own);
        }
        if let Some(level) = entry["default_reasoning_level"].as_str() {
            listed["effort"] = json!(level);
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
pub(super) fn grok_models(text: &str) -> Vec<Value> {
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
pub(super) fn agy_models(text: &str) -> Vec<Value> {
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
pub(super) fn pi_models(text: &str) -> Vec<Value> {
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
pub(super) fn codebuddy_models(text: &str) -> Vec<Value> {
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
        .enumerate()
        // The first supported model is the one CodeBuddy runs without `--model`.
        .map(|(index, id)| model(id, id, index == 0))
        .collect()
}

/// `devin models`: grouped family sections with two-space indented `<model_uid>  <label>  [...]`
/// lines, or JSON output from `devin models list --format json`.
pub(super) fn devin_models(text: &str) -> Vec<Value> {
    if text.trim_start().starts_with('{')
        && let Ok(parsed) = serde_json::from_str::<Value>(text)
        && let Some(families) = parsed["families"].as_array()
    {
        return families
            .iter()
            .filter(|family| {
                family["family_uid"]
                    .as_str()
                    .is_none_or(|id| !id.eq_ignore_ascii_case("fusion"))
            })
            .filter_map(|family| family["variants"].as_array())
            .flatten()
            .filter_map(|variant| {
                let id = variant["model_uid"].as_str()?.trim();
                (!id.is_empty() && !id.to_ascii_lowercase().starts_with("fusion-")).then(|| {
                    let label = variant["label"].as_str().map(str::trim).unwrap_or(id);
                    model(id, if label.is_empty() { id } else { label }, false)
                })
            })
            .collect();
    }
    text.lines()
        .filter_map(|line| line.strip_prefix("  "))
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let head = line.split_once("  [").map_or(line, |(head, _)| head).trim();
            let mut parts = head.split_whitespace();
            let id = parts.next()?;
            if id.to_ascii_lowercase().starts_with("fusion-") {
                return None;
            }
            let rest = head[id.len()..].trim();
            Some(model(id, if rest.is_empty() { id } else { rest }, false))
        })
        .collect()
}
