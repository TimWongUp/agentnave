//! Which provider CLIs are installed here and which models they list (ADR-0014). Probes only run
//! each CLI's own listing or help command with empty stdin, or ask a CLI that serves requests on
//! stdin for its model metadata; they never start a model turn. Each model carries the efforts
//! the CLI itself offers for it.

mod locate;
mod probe;
pub(crate) mod update;
#[cfg(unix)] // Only the Unix copy actions run a host CLI outside detection.
pub(crate) use probe::search_path;
use probe::*;

pub(crate) use locate::standalone_codex;
#[cfg(windows)]
pub(crate) use locate::store_packages;
#[cfg(windows)]
pub(crate) use locate::uninstall_icon;
use locate::*;
pub(crate) use locate::{codex_desktop_app, executable, locate};

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::providers::PROVIDERS;

/// `agy models` waits on the network and takes about 20 seconds.
const TIMEOUT: Duration = Duration::from_secs(45);
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
/// Codex: the model cache when present). Antigravity has none: each model it lists already
/// carries its level, as in `gemini-4-argon-high`.
const EFFORTS: [(&str, &[&str]); 6] = [
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
/// dsh has no listing command; its bundled official models, shipped default first.
const DEEPSEEK_CATALOG: [&str; 2] = ["deepseek-flash", "deepseek-v4-pro"];

static CACHE: LazyLock<Mutex<DetectionCache>> = LazyLock::new(|| Mutex::new(Default::default()));

#[derive(Default)]
struct DetectionCache {
    all: Option<Value>,
    revision: u64,
    // At most one successful write per provider, including during the first full probe.
    versions: BTreeMap<String, VersionWrite>,
}

struct VersionWrite {
    revision: u64,
    path: PathBuf,
    version: String,
}

impl VersionWrite {
    fn apply(&self, provider: &str, all: &mut Value) {
        if all[provider]["path"]
            .as_str()
            .is_some_and(|active| same_path(&self.path, Path::new(active)))
        {
            all[provider]["version"] = json!(self.version);
        }
    }
}

impl DetectionCache {
    fn remember(&mut self, provider: &str, path: &Path, version: &str) {
        self.revision += 1;
        let write = VersionWrite {
            revision: self.revision,
            path: path.to_owned(),
            version: version.to_owned(),
        };
        if let Some(all) = &mut self.all {
            write.apply(provider, all);
        }
        self.versions.insert(provider.to_owned(), write);
    }

    fn finish(&mut self, started: u64, mut all: Value) -> Value {
        for (provider, write) in &self.versions {
            if write.revision > started {
                write.apply(provider, &mut all);
            }
        }
        self.all = Some(all.clone());
        all
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .ok()
            .zip(b.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
}

/// Probe results for all providers, kept until `refresh` asks for a new probe. Probes run
/// outside the lock; successful updates made during a probe keep their verified versions.
/// A later probe can still observe external version changes.
pub fn detected(refresh: bool, codex_source: impl Fn() -> Option<String> + Sync) -> Value {
    detected_with(
        refresh,
        &CACHE,
        |provider| probe(provider, &codex_source),
        |all| {
            publish_native(&crate::data_files::data_root(), all);
        },
    )
}

fn detected_with(
    refresh: bool,
    cache: &Mutex<DetectionCache>,
    probe: impl Fn(&str) -> Value + Sync,
    publish: impl Fn(&Value),
) -> Value {
    let cache = || cache.lock().unwrap_or_else(|poison| poison.into_inner());
    let started = {
        let cache = cache();
        if !refresh && let Some(cached) = &cache.all {
            return cached.clone();
        }
        cache.revision
    };
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
    let all = cache().finish(started, Value::Object(all));
    publish(&all);
    all
}

/// Where the service reads each CLI's own default, so a call that names no model (and has no
/// App default) still gets the settings saved for the model that CLI will run (ADR-0028), and
/// a call that names no level passes the one the CLI would pick for its model (ADR-0042).
pub const NATIVE_FILE: &str = "cli-defaults.json";

/// The published defaults for one probe: a CLI whose probe failed keeps its last known entry,
/// one that is not installed or knows neither a default model nor any model's level drops out.
fn native_document(all: &Value, previous: &Value) -> Value {
    let mut clis = Map::new();
    for provider in PROVIDERS {
        let result = &all[provider];
        let entry = if result["installed"] == true && result["error"].is_null() {
            let efforts: Map<String, Value> = result["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|listed| {
                    let effort = listed["effort"].as_str()?;
                    Some((listed["id"].as_str()?.to_owned(), json!(effort)))
                })
                .collect();
            json!({"model": result["native"]["model"], "effort": result["native"]["effort"],
                "efforts": efforts})
        } else if result["installed"] == true {
            previous["clis"][provider].clone()
        } else {
            Value::Null
        };
        let has_efforts = entry["efforts"].as_object().is_some_and(|e| !e.is_empty());
        if entry["model"].is_string() || has_efforts {
            clis.insert((*provider).to_owned(), entry);
        }
    }
    json!({"version": 1, "clis": clis})
}

/// Write the defaults only when they changed, and never create the data root for them; a preview
/// build leaves them to the installed app.
fn publish_native(root: &Path, all: &Value) {
    if crate::PREVIEW || !root.is_dir() {
        return;
    }
    let previous: Value = std::fs::read_to_string(root.join(NATIVE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let document = native_document(all, &previous);
    if document != previous {
        let _ = crate::data_files::write_atomic(root, NATIVE_FILE, &document);
    }
}

/// The CLI's own default model from the last probe, without probing: the office names a desk
/// that requested no model by it until the CLI reports the model it used.
pub fn cached_native(provider: &str) -> Option<String> {
    let cache = CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
    cache.all.as_ref()?[provider]["native"]["model"]
        .as_str()
        .map(str::to_owned)
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
        "native": {"model": Value::Null, "effort": Value::Null},
        "efforts": efforts(provider),
    })
}

/// The CLI's own default model: the one its settings name, matched to a listed id by id or label,
/// then, both without a `[1m]`-style suffix, by id or by the model an alias resolves to; else the one
/// its listing marks as default. Never a guess: `None` when the CLI names none.
fn native_model(models: &[Value], configured: Option<&str>) -> Option<String> {
    let id = |model: &Value| model["id"].as_str().map(str::to_owned);
    fn bare(name: &str) -> &str {
        name.split('[').next().unwrap_or_default()
    }
    match configured {
        Some(name) => Some(
            models
                .iter()
                .find(|model| model["id"] == name || model["label"] == name)
                .or_else(|| {
                    models.iter().find(|model| {
                        ["id", "resolved"]
                            .iter()
                            .any(|key| model[*key].as_str().is_some_and(|v| bare(v) == bare(name)))
                    })
                })
                // A gateway may namespace the id it writes into the CLI's config
                // (`codex/gpt-6.1-terra`) while the catalog it supplies lists the bare id.
                .or_else(|| {
                    let tail = bare(name.rsplit_once('/')?.1);
                    models.iter().find(|model| {
                        ["id", "resolved"]
                            .iter()
                            .any(|key| model[*key].as_str().is_some_and(|v| bare(v) == tail))
                    })
                })
                .and_then(id)
                .unwrap_or_else(|| name.to_owned()),
        ),
        None => models
            .iter()
            .find(|model| model["default"] == true)
            .and_then(id),
    }
}

/// The default model a CLI's own settings file names, if any: Codex `config.toml` `model`,
/// Claude `settings.json` `model`, Pi `defaultModel`, and the Antigravity CLI's `model` label.
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
        ),
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
        "codebuddy" => codebuddy_settings()?["model"].as_str().map(str::to_owned),
        "devin" => {
            let config = if cfg!(windows) {
                dir("APPDATA", "AppData/Roaming")
            } else {
                dir("XDG_CONFIG_HOME", ".config")
            };
            let text = std::fs::read_to_string(config.join("devin/config.json")).ok()?;
            let value: Value = serde_json::from_str(&text).ok()?;
            value["agent"]["model"]
                .as_str()
                .or_else(|| value["model"].as_str())
                .map(str::to_owned)
        }
        _ => None,
    }?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// The thinking level a CLI's own settings name for a model, if any: Codex `config.toml`
/// `model_reasoning_effort`, Claude `settings.json` `env.CLAUDE_CODE_EFFORT_LEVEL` then
/// `effortLevel` (per model first; the top one only for the models it applies to), Pi
/// `modelThinkingLevels` then `defaultThinkingLevel`, CodeBuddy `reasoningEffort`, and Grok
/// `config.toml` `[model.<id>] reasoning_effort`, then for its default model `[models]
/// default_reasoning_effort` and the level its model cache marks (dsh's comes from its dump;
/// Antigravity's is part of the model).
fn configured_effort(provider: &str, model: Option<&str>, native: bool) -> Option<String> {
    let home = dirs::home_dir()?;
    let dir = |variable: &str, fallback: &str| match std::env::var_os(variable) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home.join(fallback),
    };
    let effort = match provider {
        "codex" => {
            let text = std::fs::read_to_string(dir("CODEX_HOME", ".codex").join("config.toml"));
            let table: toml::Table = text.ok()?.parse().ok()?;
            table.get("model_reasoning_effort")?.as_str()?.to_owned()
        }
        "claude" => {
            let path = dir("CLAUDE_CONFIG_DIR", ".claude").join("settings.json");
            let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
            // The env level outranks every saved one in each session it starts.
            let name = model.map(|model| claude_name(model).unwrap_or_else(|| model.to_owned()));
            let per_model = name.map(|name| &value["modelSettings"][name.as_str()]["effortLevel"]);
            let top = model
                .is_none_or(claude_reads_top)
                .then(|| value["effortLevel"].as_str())
                .flatten();
            value["env"]["CLAUDE_CODE_EFFORT_LEVEL"]
                .as_str()
                .or_else(|| per_model.and_then(Value::as_str))
                .or(top)?
                .to_owned()
        }
        "pi" => {
            let path = dir("PI_CODING_AGENT_DIR", ".pi/agent").join("settings.json");
            let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
            let per_model = model.map(|model| &value["modelThinkingLevels"][model]);
            per_model
                .and_then(Value::as_str)
                .or_else(|| value["defaultThinkingLevel"].as_str())?
                .to_owned()
        }
        "codebuddy" => codebuddy_settings()?["reasoningEffort"]
            .as_str()?
            .to_owned(),
        "grok" => {
            let model = model?;
            let text = std::fs::read_to_string(home.join(".grok/config.toml")).unwrap_or_default();
            let table: toml::Table = text.parse().unwrap_or_default();
            let per_model = table.get("model").and_then(|models| models.get(model));
            let default = native
                .then(|| table.get("models")?.get("default_reasoning_effort"))
                .flatten();
            let cache = || {
                let text = std::fs::read_to_string(home.join(".grok/models_cache.json")).ok()?;
                grok_default_effort(&serde_json::from_str(&text).ok()?, model)
            };
            per_model
                .and_then(|entry| entry.get("reasoning_effort"))
                .or(default)
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
                .or_else(|| native.then(cache).flatten())?
        }
        _ => return None,
    };
    let effort = effort.trim();
    (!effort.is_empty()).then(|| effort.to_owned())
}

/// The Claude models the top-level `effortLevel` still applies to: Opus 5.5 and every later or
/// unknown Claude read only `modelSettings.<model>.effortLevel`; another vendor's model or an
/// unresolved alias reads the top one.
const CLAUDE_TOP_EFFORT: [&str; 13] = [
    "claude-opus-4",
    "claude-sonnet-4",
    "claude-opus-4-5",
    "claude-sonnet-4-5",
    "claude-haiku-4-5",
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-opus-4-7",
    "claude-opus-4-8",
    "claude-opus-5",
    "claude-sonnet-5",
    "claude-fable-5",
    "claude-fable-5-1",
];

/// The Claude model an ID names as `claude-<family>-<major>[-<minor>]`, a dated or dotted ID
/// or a `[1m]` suffix included; None for an alias or another vendor's model.
fn claude_name(model: &str) -> Option<String> {
    let model = model.to_lowercase().replace('.', "-");
    let model = model.split('[').next().unwrap_or_default();
    let (_, rest) = model.split_once("claude-")?;
    let mut parts = rest.split('-');
    let family = parts
        .next()
        .filter(|family| ["opus", "sonnet", "haiku", "fable"].contains(family))?;
    let digits = |part: &&str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let major = parts.next().filter(digits)?;
    let mut name = format!("claude-{family}-{major}");
    if let Some(minor) = parts.next().filter(|part| digits(part) && part.len() <= 2) {
        name.push('-');
        name.push_str(minor);
    }
    Some(name)
}

/// An alias its picker did not resolve could be any model, so the level stays unknown.
fn claude_reads_top(model: &str) -> bool {
    !CLAUDE_ALIASES.contains(&model)
        && claude_name(model).is_none_or(|name| CLAUDE_TOP_EFFORT.contains(&name.as_str()))
}

/// The reasoning effort Grok's model cache marks as default for one model.
fn grok_default_effort(cache: &Value, model: &str) -> Option<String> {
    cache["models"][model]["info"]["reasoning_efforts"]
        .as_array()?
        .iter()
        .find(|effort| effort["default"] == true)?["value"]
        .as_str()
        .map(str::to_owned)
}

/// CodeBuddy's config folder.
fn codebuddy_dir() -> Option<PathBuf> {
    match std::env::var_os("CODEBUDDY_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(".codebuddy")),
    }
}

/// CodeBuddy's `settings.json` in its config folder.
fn codebuddy_settings() -> Option<Value> {
    let path = codebuddy_dir()?.join("settings.json");
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Pi's thinking levels, lowest first: a level a model lacks runs at the next one up it has,
/// else the next one down.
const PI_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

fn pi_level(wanted: &str, offered: &[String]) -> Option<String> {
    let at = PI_LEVELS.iter().position(|level| *level == wanted)?;
    let has = |level: &&&str| offered.iter().any(|own| own == **level);
    PI_LEVELS[at..]
        .iter()
        .find(has)
        .or_else(|| PI_LEVELS[..at].iter().rev().find(has))
        .map(|level| (*level).to_owned())
}

/// Each model's `effort`: the level its CLI runs it at when a call names none. The CLI's own
/// setting for the model wins where the model offers it (Pi's falling back to Pi's `medium`
/// and moving to a level the model has), else the level the CLI's listing marks for the model;
/// unknown stays unset.
fn model_efforts(
    provider: &str,
    models: &mut [Value],
    levels: &[String],
    setting: impl Fn(&str) -> Option<String>,
) {
    for listed in models {
        let offered: Vec<String> = match listed["efforts"].as_array() {
            Some(own) => own
                .iter()
                .filter_map(|l| l.as_str().map(str::to_owned))
                .collect(),
            None => levels.to_vec(),
        };
        let setting = setting(listed["id"].as_str().unwrap_or_default());
        let setting = match provider {
            "pi" => pi_level(setting.as_deref().unwrap_or("medium"), &offered),
            _ => setting,
        };
        let own = listed["effort"].as_str().map(str::to_owned);
        let effort = [setting, own]
            .into_iter()
            .flatten()
            .find(|level| offered.contains(level));
        match effort {
            Some(level) => listed["effort"] = json!(level),
            None => {
                if let Some(entry) = listed.as_object_mut() {
                    entry.remove("effort");
                }
            }
        }
    }
}

/// One plain, single- or double-quoted YAML scalar from `dsh --dump-config`.
fn dsh_scalar(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let value = if raw.starts_with('"') {
        serde_json::from_str::<String>(raw).ok()
    } else if raw.len() > 1 && raw.starts_with('\'') && raw.ends_with('\'') {
        Some(raw[1..raw.len() - 1].replace("''", "'"))
    } else if raw.starts_with(['!', '&', '*', '[', '{', '|', '>', '\'']) {
        // A tagged/dynamic value, alias or compound scalar is not a known identity.
        None
    } else {
        Some(raw.to_owned())
    };
    value.filter(|value| !value.is_empty())
}

/// A key (`model`, `reasoningEffort`) of the `agent-default-model` entry in `dsh --dump-config`
/// output: the composed headless profile, so a user layer that routes dsh elsewhere wins.
fn dsh_default(dump: &str, key: &str) -> Option<String> {
    let prefix = format!("    {key}:");
    let mut inside = false;
    let mut found = None;
    for line in dump.lines() {
        if line.starts_with("- ") || line.starts_with('#') {
            inside = line.trim() == "- id: agent-default-model";
            if inside {
                found = None;
            }
        } else if inside && let Some(raw) = line.strip_prefix(&prefix) {
            found = dsh_scalar(raw);
        }
    }
    found
}

/// The models the configured providers declare (`config.providers.<route>.models[].id`), in
/// order: what a model override may name besides the bundled official models. Each comes with
/// the keys of its `reasoningEfforts`; a model without them does not reason and has none. A model
/// declared again keeps its first levels.
fn dsh_provider_models(dump: &str) -> Vec<(String, Vec<String>)> {
    let mut providers = false;
    let mut levels = false;
    let mut models: Vec<(String, Vec<String>)> = Vec::new();
    let mut current = None;
    for line in dump.lines() {
        if line.starts_with("- ") || line.starts_with('#') {
            providers = false;
        } else if line.starts_with("    ") && !line.starts_with("     ") {
            providers = line.trim_end() == "    providers:";
        } else if !providers {
            continue;
        } else if let Some(id) = line.strip_prefix("          - id:").and_then(dsh_scalar) {
            levels = false;
            current = (!models.iter().any(|(known, _)| *known == id)).then_some(models.len());
            if current.is_some() {
                models.push((id, Vec::new()));
            }
        } else if line.starts_with("            ") && !line.starts_with("             ") {
            levels = line.trim_end() == "            reasoningEfforts:";
        } else if levels
            && line.starts_with("              ")
            && !line.starts_with("               ")
            && let Some((level, _)) = line.trim().split_once(':')
            && let Some((_, efforts)) = current.and_then(|at| models.get_mut(at))
        {
            efforts.push(level.to_owned());
        } else if !line.starts_with("              ") {
            levels = false;
        }
    }
    models
}

/// ZCode's providers' models, each with the `reasoningLevel` values its model rule offers.
fn zcode_models(document: &Value) -> Vec<Value> {
    let config = &document["config"];
    let levels = |qualified: &str| {
        config["modelConfigRules"]["providerModelRules"]
            .as_array()?
            .iter()
            .find(|rule| {
                format!(
                    "{}/{}",
                    rule["providerId"].as_str().unwrap_or(""),
                    rule["modelId"].as_str().unwrap_or("")
                ) == qualified
            })?["config"]["optionSpecs"]["reasoningLevel"]["values"]
            .as_array()
            .cloned()
    };
    let mut models = Vec::new();
    if let Some(rules) = document["config"]["providerConfigRules"]["providerRules"].as_array() {
        for rule in rules {
            let Some(provider) = rule["providerId"].as_str() else {
                continue;
            };
            for key in ["modelOrder", "personalModelIds"] {
                if let Some(ids) = rule["config"][key].as_array() {
                    for id in ids.iter().filter_map(Value::as_str) {
                        let qualified = format!("{provider}/{id}");
                        if !models.iter().any(|m: &Value| m["id"] == qualified) {
                            let mut listed = model(&qualified, &qualified, false);
                            if let Some(efforts) = levels(&qualified) {
                                listed["efforts"] = Value::Array(efforts);
                            }
                            models.push(listed);
                        }
                    }
                }
            }
        }
    }
    models
}

fn probe_zcode() -> Value {
    let path = find("zcode").or_else(|| {
        if !cfg!(target_os = "macos") {
            return None;
        }
        [
            Some(PathBuf::from("/Applications/ZCode.app")),
            dirs::home_dir().map(|h| h.join("Applications/ZCode.app")),
        ]
        .into_iter()
        .flatten()
        .map(|p| p.join("Contents/Resources/glm/zcode.cjs"))
        .find(|p| p.is_file())
    });
    let Some(path) = path else {
        return failure("zcode", None, "");
    };
    let models = dirs::home_dir()
        .and_then(|h| std::fs::read_to_string(h.join(".zcode/v2/provider_config.json")).ok())
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .map(|d| zcode_models(&d))
        .unwrap_or_default();
    json!({"installed":true,"path":path,"source":"zcode-config","error":null,
        "native":{"model":null,"effort":null},"models":models,"efforts":[]})
}

fn probe(provider: &str, codex_source: &impl Fn() -> Option<String>) -> Value {
    if provider == "zcode" {
        return probe_zcode();
    }
    let binary = match provider {
        "antigravity" => "agy",
        "deepseek" => "dsh",
        other => other,
    };
    let selected = if provider == "codex" {
        codex_source()
    } else {
        None
    };
    // A missing chosen install still finds the other one, so the card stays where its source can
    // be changed; runs refuse the missing source.
    let path = match selected.as_deref() {
        Some("desktop") => codex_desktop(),
        Some("standalone") => standalone_codex(),
        _ => None,
    }
    .or_else(|| find(binary));
    let Some(path) = path else {
        return failure(provider, None, "");
    };
    let mut result = probe_path(provider, &path);
    result["version"] = run(&path, &["--version"])
        .ok()
        .and_then(|out| version_of(&out))
        .map_or(Value::Null, Value::from);
    result
}

/// The first version-looking word on the first line of `--version` output: `codex-cli 0.161.0`,
/// `2.1.308 (Claude Code)`, `grok 1.0.46 (2765805b)` all give the number; later lines (a
/// runtime's own version, say) are never read.
fn version_of(output: &str) -> Option<String> {
    let first = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    first.split_whitespace().find_map(|word| {
        let word = word.trim_start_matches('v');
        let end = word
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')))
            .unwrap_or(word.len());
        let version = &word[..end];
        (version.starts_with(|c: char| c.is_ascii_digit()) && version.contains('.'))
            .then(|| version.to_owned())
    })
}

fn probe_path(provider: &str, path: &Path) -> Value {
    let mut efforts = efforts(provider);
    let dsh_probe =
        (provider == "deepseek").then(|| run(path, &["--profile", "headless", "--dump-config"]));
    let dsh_dump = dsh_probe.as_ref().and_then(|result| result.as_ref().ok());
    let dsh_error = dsh_probe.as_ref().and_then(|result| result.as_ref().err());
    let configured_model = match provider {
        "deepseek" => dsh_dump.and_then(|dump| dsh_default(dump, "model")),
        _ => configured(provider),
    };
    let dsh_effort = dsh_dump.and_then(|dump| dsh_default(dump, "reasoningEffort"));
    let listed = listed_models(
        provider,
        path,
        &mut efforts,
        dsh_dump,
        configured_model.as_ref(),
    );
    match listed {
        Ok((source, models)) => {
            let native = native_model(&models, configured_model.as_deref());
            let mut models = models;
            let resolved: Vec<(String, String)> = models
                .iter()
                .filter_map(|listed| {
                    Some((
                        listed["id"].as_str()?.to_owned(),
                        listed["resolved"].as_str()?.to_owned(),
                    ))
                })
                .collect();
            model_efforts(provider, &mut models, &efforts, |id| {
                let full = resolved
                    .iter()
                    .find(|(alias, _)| alias == id)
                    .map_or(id, |(_, full)| full.as_str());
                match provider {
                    "deepseek" => dsh_effort.clone(),
                    "codex" => configured_effort(provider, None, false),
                    _ => configured_effort(provider, Some(full), Some(id) == native.as_deref()),
                }
            });
            // CodeBuddy thinks only on request when `alwaysThinkingEnabled` is off, whatever
            // `reasoningEffort` says.
            let thinking_off = provider == "codebuddy"
                && codebuddy_settings()
                    .is_some_and(|settings| settings["alwaysThinkingEnabled"] == false);
            if thinking_off {
                for listed in &mut models {
                    listed.as_object_mut().map(|entry| entry.remove("effort"));
                }
            }
            // A listed default model runs at its own level; one the listing lacks at the setting.
            let effort = match &native {
                Some(id) => match models.iter().find(|listed| listed["id"] == id.as_str()) {
                    Some(listed) => listed["effort"].as_str().map(str::to_owned),
                    None => dsh_effort
                        .clone()
                        .or_else(|| configured_effort(provider, Some(id), true))
                        .filter(|_| !thinking_off),
                },
                None => dsh_effort.clone(),
            };
            json!({
            "installed": true,
            "path": path.display().to_string(),
            "source": source,
            "error": dsh_error,
            "native": {"model": native, "effort": effort},
            "models": models,
            "efforts": efforts,
            })
        }
        Err(error) => failure(provider, Some(path), &error),
    }
}

fn listed_models(
    provider: &str,
    path: &Path,
    efforts: &mut Vec<String>,
    dsh_dump: Option<&String>,
    configured_model: Option<&String>,
) -> Result<(&'static str, Vec<Value>), String> {
    match provider {
        "codex" => codex_models().map(|(models, levels)| {
            if !levels.is_empty() {
                *efforts = levels;
            }
            ("codex-cache", models)
        }),
        // Its own model picker; nothing stands in when it cannot answer.
        "claude" => claude_models(path).map(|models| ("initialize", models)),
        "deepseek" => deepseek_listing(dsh_dump, configured_model),
        "grok" => run(path, &["models"]).map(|out| {
            let cache = dirs::home_dir()
                .and_then(|home| std::fs::read_to_string(home.join(".grok/models_cache.json")).ok())
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .unwrap_or(Value::Null);
            let mut models = grok_models(&out);
            for listed in &mut models {
                let id = listed["id"].as_str().unwrap_or("").to_owned();
                if let Some(efforts) = grok_efforts(&cache, &id) {
                    listed["efforts"] = json!(efforts);
                }
                if let Some(effort) = grok_default_effort(&cache, &id) {
                    listed["effort"] = json!(effort);
                }
            }
            ("command", models)
        }),
        "antigravity" => run(path, &["models"]).map(|out| ("command", agy_models(&out))),
        "devin" => run(path, &["models", "list", "--format", "json"])
            .map(|out| ("command", devin_models(&out))),
        // Pi answers over RPC with each model's thinking levels; the listing has no levels.
        "pi" => pi_rpc_models(path)
            .map(|models| ("rpc", models))
            .or_else(|_| {
                run_command(login_command(path, &["--list-models"]))
                    .map(|out| ("command", pi_models(&out)))
            })
            .and_then(|(source, models)| {
                if models.is_empty() {
                    Err("Pi 没有列出可用模型，请先运行 pi /login 或配置 auth.json".to_owned())
                } else {
                    Ok((source, models))
                }
            }),
        "codebuddy" => run(path, &["--help"]).map(|out| {
            let mut models = codebuddy_models(&out);
            let reasoning = codebuddy_reasoning();
            for listed in &mut models {
                let Some(own) = listed["id"].as_str().and_then(|id| reasoning.get(id)) else {
                    continue;
                };
                // CodeBuddy offers every level for a model that lists none.
                if own["supportedEfforts"]
                    .as_array()
                    .is_some_and(|l| !l.is_empty())
                {
                    listed["efforts"] = own["supportedEfforts"].clone();
                }
                if let Some(effort) = own["defaultEffort"].as_str().or(own["effort"].as_str()) {
                    listed["effort"] = json!(effort);
                }
            }
            ("help", models)
        }),
        _ => Err("未知 CLI".to_owned()),
    }
}

fn deepseek_listing(
    dsh_dump: Option<&String>,
    configured_model: Option<&String>,
) -> Result<(&'static str, Vec<Value>), String> {
    // The composed profile's default first, then the official models and the ones
    // its configured providers (a gateway, say) declare.
    let mut ids: Vec<String> = Vec::new();
    let declared_efforts = dsh_dump
        .map(|dump| dsh_provider_models(dump))
        .unwrap_or_default();
    let declared = declared_efforts.iter().map(|(id, _)| id.clone());
    for id in configured_model
        .into_iter()
        .cloned()
        .chain(DEEPSEEK_CATALOG.iter().map(|id| (*id).to_owned()))
        .chain(declared)
    {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let models = ids
        .iter()
        .map(|id| {
            let mut listed = model(id, id, configured_model == Some(id));
            // A declared model offers the levels it declares; the official ones keep
            // the route's levels.
            if let Some((_, efforts)) = declared_efforts.iter().find(|(m, _)| m == id) {
                listed["efforts"] = json!(efforts);
            }
            listed
        })
        .collect();
    let source = if dsh_dump.is_some() {
        "config"
    } else {
        "catalog"
    };
    Ok((source, models))
}

#[cfg(test)]
mod tests;
