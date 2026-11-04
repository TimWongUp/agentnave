//! The user's `members.json`: which model + CLI pairs each mode may dispatch, and their
//! defaults. The tray is its only writer; Python servers read it on every call. Checks here are
//! structural and referential only; Python Adapters validate option semantics.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};

pub const PROVIDERS: [&str; 8] = [
    "zcode",
    "antigravity",
    "claude",
    "codebuddy",
    "codex",
    "deepseek",
    "grok",
    "pi",
];
const MODES: [&str; 2] = ["task", "discussion"];
/// The agentnave-review roles whose CLI and model the user may pick.
pub const REVIEW_ROLES: [&str; 9] = [
    "correctness-reviewer",
    "correctness-reviewer-easy",
    "architecture-reviewer",
    "security-reviewer",
    "performance-reviewer",
    "test-reliability-reviewer",
    "simplicity-reviewer",
    "specialist-reviewer",
    "adversarial-verifier",
];
const FILE: &str = "members.json";
const REVIEW_FILE: &str = "review.json";
const PERMISSIONS_FILE: &str = "permissions.json";
const ORCHESTRATION_FILE: &str = "orchestration.json";
/// Per-CLI permission tiers; the server maps each to that CLI's own options. Pi has none.
const TIERS: [&str; 2] = ["auto", "full"];

pub fn empty() -> Value {
    json!({"version": 1, "clis": {}, "modes": {}})
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{what} 必须为对象"))
}

fn only(fields: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<(), String> {
    match fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(format!("{what} 含未知字段 {key}")),
        None => Ok(()),
    }
}

/// `efforts`, `effort`, `options` and `notes`, shared by a model and by a mode override.
fn settings(value: &Value, what: &str) -> Result<(), String> {
    let fields = object(value, what)?;
    only(fields, &["efforts", "effort", "options", "notes"], what)?;
    if let Some(efforts) = fields.get("efforts") {
        let list = efforts
            .as_array()
            .ok_or_else(|| format!("{what} 的 efforts 必须为数组"))?;
        if !list.iter().all(Value::is_string) {
            return Err(format!("{what} 的 efforts 只能含字符串"));
        }
    }
    if fields
        .get("effort")
        .is_some_and(|effort| !effort.is_string())
    {
        return Err(format!("{what} 的 effort 必须为字符串"));
    }
    if let Some(options) = fields.get("options") {
        let options = object(options, &format!("{what} 的 options"))?;
        if options.contains_key("model") || options.contains_key("effort") {
            return Err(format!("{what} 的 options 不能含 model 或 effort"));
        }
        if options
            .values()
            .any(|value| !(value.is_string() || value.is_number() || value.is_boolean()))
        {
            return Err(format!("{what} 的参数值只支持字符串、数字或布尔值"));
        }
    }
    if fields.get("notes").is_some_and(|notes| !notes.is_string()) {
        return Err(format!("{what} 的 notes 必须为字符串"));
    }
    Ok(())
}

/// The default effort must be one of the allowed efforts, after a mode override replaced fields.
fn effort_allowed(shared: &Value, modal: Option<&Value>, what: &str) -> Result<(), String> {
    let pick = |key: &str| {
        modal
            .and_then(|modal| modal.get(key))
            .or_else(|| shared.get(key))
    };
    let efforts = pick("efforts").and_then(Value::as_array);
    if let (Some(effort), Some(efforts)) = (pick("effort"), efforts)
        && !efforts.is_empty()
        && !efforts.contains(effort)
    {
        return Err(format!("{what} 的默认 effort 不在允许的 efforts 中"));
    }
    Ok(())
}

pub fn validate(document: &Value) -> Result<(), String> {
    let top = object(document, "members.json")?;
    only(
        top,
        &["version", "clis", "modes", "defaults"],
        "members.json",
    )?;
    if document["version"].as_u64() != Some(1) {
        return Err("members.json 的 version 必须为 1".into());
    }
    let (Some(clis), Some(modes)) = (top.get("clis"), top.get("modes")) else {
        return Err("members.json 必须含 version、clis 与 modes".into());
    };
    let empty = Map::new();
    let clis = object(clis, "clis")?;
    validate_clis(clis)?;
    let modes = object(modes, "modes")?;
    for (mode, members) in modes {
        if !MODES.contains(&mode.as_str()) {
            return Err(format!("未知模式：{mode}"));
        }
        for (provider, member) in object(members, &format!("modes.{mode}"))? {
            let what = format!("modes.{mode}.{provider}");
            if !PROVIDERS.contains(&provider.as_str()) {
                return Err(format!("不支持的 CLI：{provider}"));
            }
            let fields = object(member, &what)?;
            only(fields, &["models", "default", "overrides"], &what)?;
            let known = clis
                .get(provider)
                .and_then(|cli| cli.get("models"))
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            let mut listed = Vec::new();
            {
                let models = fields
                    .get("models")
                    .ok_or_else(|| format!("{what} 缺少 models（空列表表示不限模型）"))?
                    .as_array()
                    .ok_or_else(|| format!("{what}.models 必须为数组"))?;
                for model in models {
                    let model = model
                        .as_str()
                        .ok_or_else(|| format!("{what}.models 只能含字符串"))?;
                    if !known.contains_key(model) {
                        return Err(format!("{what} 引用了未配置的模型 {model}"));
                    }
                    listed.push(model);
                }
            }
            if let Some(default) = fields.get("default") {
                let default = default
                    .as_str()
                    .ok_or_else(|| format!("{what}.default 必须为字符串"))?;
                if !known.contains_key(default) || !(listed.is_empty() || listed.contains(&default))
                {
                    return Err(format!("{what} 的默认模型 {default} 不在允许的模型中"));
                }
            }
            if let Some(overrides) = fields.get("overrides") {
                for (model, modal) in object(overrides, &format!("{what}.overrides"))? {
                    // Like the server, an override only needs a configured model.
                    if !known.contains_key(model) {
                        return Err(format!("{what} 覆盖了未配置的模型 {model}"));
                    }
                    let what = format!("{what} / {model}");
                    settings(modal, &what)?;
                    effort_allowed(&known[model], Some(modal), &what)?;
                }
            }
        }
    }
    // Default models kept for unrestricted modes after the user turned the allow-list off.
    if let Some(defaults) = top.get("defaults") {
        for (mode, members) in object(defaults, "defaults")? {
            if !MODES.contains(&mode.as_str()) {
                return Err(format!("未知模式：{mode}"));
            }
            if modes.contains_key(mode) {
                return Err(format!("defaults.{mode} 只用于未自选的场景"));
            }
            for (provider, model) in object(members, &format!("defaults.{mode}"))? {
                let what = format!("defaults.{mode}.{provider}");
                let model = model
                    .as_str()
                    .ok_or_else(|| format!("{what} 必须为字符串"))?;
                let known = clis
                    .get(provider)
                    .and_then(|cli| cli.get("models"))
                    .and_then(Value::as_object)
                    .unwrap_or(&empty);
                if !known.contains_key(model) {
                    return Err(format!("{what} 引用了未配置的模型 {model}"));
                }
            }
        }
    }
    Ok(())
}

pub fn read(root: &Path) -> Result<Option<Value>, String> {
    let content = match fs::read(root.join(FILE)) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法读取 {FILE}：{error}")),
    };
    let document =
        serde_json::from_slice(&content).map_err(|error| format!("{FILE} 损坏：{error}"))?;
    validate(&document).map_err(|error| format!("{FILE} 格式错误：{error}"))?;
    Ok(Some(document))
}

/// Replace `name` under `root` atomically: a unique sibling file, synced, then renamed.
pub(crate) fn write_atomic(root: &Path, name: &str, document: &Value) -> Result<(), String> {
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(root)?;
        // Dropping the temporary file cleans it up on failure, leaving the old file intact.
        let mut temporary = tempfile::NamedTempFile::new_in(root)?;
        temporary.write_all(&serde_json::to_vec_pretty(document)?)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist(root.join(name))?;
        Ok(())
    };
    write().map_err(|error| format!("无法保存 {name}：{error}"))
}

pub fn save(root: &Path, document: &Value) -> Result<(), String> {
    validate(document)?;
    write_atomic(root, FILE, document)
}

/// review.json: `{"version": 1, "seats": {role: {provider, model, effort?}}}`. Kept out of
/// members.json so a server that predates review seats still reads its members.
/// Seats stay structural only: a seat whose model was removed later is reported by the server
/// and replaced when the review Skill dispatches.
pub fn validate_review(document: &Value) -> Result<(), String> {
    let top = object(document, REVIEW_FILE)?;
    only(top, &["version", "seats"], REVIEW_FILE)?;
    if document["version"].as_u64() != Some(1) {
        return Err(format!("{REVIEW_FILE} 的 version 必须为 1"));
    }
    for (role, seat) in object(&document["seats"], "seats")? {
        if !REVIEW_ROLES.contains(&role.as_str()) {
            return Err(format!("未知评审角色：{role}"));
        }
        validate_seat(seat, &format!("seats.{role}"))?;
    }
    Ok(())
}

/// One picked CLI and model, with an optional effort; shared by review and orchestration seats.
fn validate_seat(seat: &Value, what: &str) -> Result<(), String> {
    let fields = object(seat, what)?;
    only(fields, &["provider", "model", "effort"], what)?;
    if !fields
        .get("provider")
        .and_then(Value::as_str)
        .is_some_and(|provider| PROVIDERS.contains(&provider))
    {
        return Err(format!("{what} 的 CLI 不受支持"));
    }
    if !fields
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| !model.trim().is_empty())
    {
        return Err(format!("{what} 缺少模型"));
    }
    if fields
        .get("effort")
        .is_some_and(|effort| !effort.is_string())
    {
        return Err(format!("{what} 的 effort 必须为字符串"));
    }
    Ok(())
}

/// orchestration.json: `{"version": 1, "preset": "off" | "split" | "parallel" | "custom",
/// "custom": text, "apps": {row: mode}, "effective_apps": {row: mode}, "explorer": {"easy" | "hard":
/// seat}, "worker": {...}, "worker_frontend": {...}}`. Interface Workers have a key of their own so
/// servers that predate it ignore them rather than reject the file. The App writes the chosen
/// rules into the boss apps' global rules and
/// `effective_apps`, every connected row's mode as its group applies it, for the server.
pub fn validate_orchestration(document: &Value) -> Result<(), String> {
    let top = object(document, ORCHESTRATION_FILE)?;
    only(
        top,
        &[
            "version",
            "preset",
            "custom",
            "apps",
            "effective_apps",
            "explorer",
            "worker",
            "worker_frontend",
        ],
        ORCHESTRATION_FILE,
    )?;
    if document["version"].as_u64() != Some(1) {
        return Err(format!("{ORCHESTRATION_FILE} 的 version 必须为 1"));
    }
    match top.get("custom") {
        None => {}
        Some(Value::String(text)) => crate::rules::check_custom(text)?,
        Some(_) => return Err("自定义编排必须为文本".into()),
    }
    let mut modes = vec![&document["preset"]];
    for key in ["apps", "effective_apps"] {
        if let Some(apps) = top.get(key) {
            modes.extend(object(apps, key)?.values());
        }
    }
    if !modes.iter().all(|mode| {
        mode.as_str()
            .is_some_and(|mode| crate::rules::PRESETS.contains(&mode))
    }) {
        return Err("编排档位无效".into());
    }
    if modes.iter().any(|mode| *mode == "custom")
        && !document["custom"]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty())
    {
        return Err("自定义编排不能为空".into());
    }
    for role in ["explorer", "worker", "worker_frontend"] {
        let Some(seats) = top.get(role) else { continue };
        for (difficulty, seat) in object(seats, role)? {
            if !matches!(difficulty.as_str(), "easy" | "hard") {
                return Err(format!("{role} 只有 easy 与 hard 两档"));
            }
            validate_seat(seat, &format!("{role}.{difficulty}"))?;
        }
    }
    Ok(())
}

/// The saved orchestration settings, or "off" with no seats when none was written yet.
pub fn read_orchestration(root: &Path) -> Result<Value, String> {
    let content = match fs::read(root.join(ORCHESTRATION_FILE)) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(json!({"version": 1, "preset": "off"}));
        }
        Err(error) => return Err(format!("无法读取 {ORCHESTRATION_FILE}：{error}")),
    };
    let document = serde_json::from_slice(&content)
        .map_err(|error| format!("{ORCHESTRATION_FILE} 损坏：{error}"))?;
    validate_orchestration(&document)
        .map_err(|error| format!("{ORCHESTRATION_FILE} 格式错误：{error}"))?;
    Ok(document)
}

pub fn save_orchestration(root: &Path, document: &Value) -> Result<(), String> {
    validate_orchestration(document)?;
    write_atomic(root, ORCHESTRATION_FILE, document)
}

/// The saved review seats, or an empty file when none was written yet.
pub fn read_review(root: &Path) -> Result<Value, String> {
    let content = match fs::read(root.join(REVIEW_FILE)) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(json!({"version": 1, "seats": {}}));
        }
        Err(error) => return Err(format!("无法读取 {REVIEW_FILE}：{error}")),
    };
    let document =
        serde_json::from_slice(&content).map_err(|error| format!("{REVIEW_FILE} 损坏：{error}"))?;
    validate_review(&document).map_err(|error| format!("{REVIEW_FILE} 格式错误：{error}"))?;
    Ok(document)
}

pub fn save_review(root: &Path, document: &Value) -> Result<(), String> {
    validate_review(document)?;
    write_atomic(root, REVIEW_FILE, document)
}

/// permissions.json: `{"version": 1, "clis": {provider: "auto" | "full"}}`. Kept out of
/// members.json so a server that predates permission tiers still reads its members. A missing
/// file or CLI runs "auto": a dispatched CLI cannot ask anyone, so its native defaults deny.
pub fn validate_permissions(document: &Value) -> Result<(), String> {
    let top = object(document, PERMISSIONS_FILE)?;
    only(top, &["version", "clis"], PERMISSIONS_FILE)?;
    if document["version"].as_u64() != Some(1) {
        return Err(format!("{PERMISSIONS_FILE} 的 version 必须为 1"));
    }
    for (provider, tier) in object(&document["clis"], "clis")? {
        if !PROVIDERS.contains(&provider.as_str()) || provider == "pi" {
            return Err(format!("{provider} 不支持权限档位"));
        }
        if !tier.as_str().is_some_and(|tier| TIERS.contains(&tier)) {
            return Err(format!("{provider} 的权限档位只能是 auto 或 full"));
        }
    }
    Ok(())
}

pub fn read_permissions(root: &Path) -> Result<Option<Value>, String> {
    let content = match fs::read(root.join(PERMISSIONS_FILE)) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法读取 {PERMISSIONS_FILE}：{error}")),
    };
    let document = serde_json::from_slice(&content)
        .map_err(|error| format!("{PERMISSIONS_FILE} 损坏：{error}"))?;
    validate_permissions(&document)
        .map_err(|error| format!("{PERMISSIONS_FILE} 格式错误：{error}"))?;
    Ok(Some(document))
}

pub fn save_permissions(root: &Path, document: &Value) -> Result<(), String> {
    validate_permissions(document)?;
    write_atomic(root, PERMISSIONS_FILE, document)
}

/// Freeze previously unrestricted scenes at the installed CLIs, keeping their defaults.
fn confirm_scenes(document: &mut Value, detected: &Value) -> bool {
    let before = document.clone();
    for mode in MODES {
        if document["modes"].get(mode).is_some() {
            continue;
        }
        let kept = document["defaults"][mode]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut selection = Map::new();
        for provider in PROVIDERS {
            if detected[provider]["installed"] != true && !kept.contains_key(provider) {
                continue;
            }
            let mut member = json!({"models": [], "overrides": {}});
            if let Some(model) = kept.get(provider) {
                member["default"] = model.clone();
            }
            selection.insert(provider.into(), member);
        }
        document["modes"][mode] = Value::Object(selection);
        if let Some(defaults) = document.get_mut("defaults").and_then(Value::as_object_mut) {
            defaults.remove(mode);
        }
    }
    if document["defaults"].as_object().is_some_and(Map::is_empty) {
        document.as_object_mut().unwrap().remove("defaults");
    }
    *document != before
}

fn migrate(root: &Path, detected: &Value, has_boss: bool) -> Result<(), String> {
    if crate::PREVIEW {
        return Ok(());
    }
    let mut document = match read(root)? {
        Some(document) => document,
        None if has_boss => empty(),
        None => return Ok(()),
    };
    if confirm_scenes(&mut document, detected) {
        save(root, &document)?;
    }
    Ok(())
}

/// Whether AgentNave ran here before: a boss was connected or has dispatched work. A missing
/// file then meant every CLI, so the migration keeps them.
pub fn used_before(root: &Path, hosts: &Value) -> bool {
    // A boss counts once its config carries the AgentNave entry, even while it needs repair.
    let connected = hosts["hosts"].as_array().is_some_and(|hosts| {
        hosts.iter().any(|host| {
            host["status"] == "connected"
                || host["layers"]
                    .as_array()
                    .is_some_and(|layers| layers.iter().any(|layer| layer["has_entry"] == true))
        })
    });
    connected
        || crate::db::read(root).ok().flatten().is_some_and(|db| {
            db.query_row("SELECT EXISTS (SELECT 1 FROM conversations)", [], |row| {
                row.get(0)
            })
            .unwrap_or(false)
        })
}

/// Run once after host detection, while host actions are locked out of the migration.
pub fn prepare(root: &Path, hosts: &Value, detected: &Value) {
    if let Err(error) = migrate(root, detected, used_before(root, hosts)) {
        eprintln!("无法迁移员工名单：{error}");
    }
}

/// The employee page waits for the start-up migration, so it never edits the list from before.
static PREPARED: Mutex<bool> = Mutex::new(false);
static PREPARED_CHANGED: Condvar = Condvar::new();

/// Called once the start-up migration ran or was skipped.
pub fn mark_prepared() {
    *PREPARED.lock().unwrap_or_else(|p| p.into_inner()) = true;
    PREPARED_CHANGED.notify_all();
}

fn wait_prepared() {
    if cfg!(test) {
        return;
    }
    let ready = PREPARED.lock().unwrap_or_else(|p| p.into_inner());
    // Probing the CLIs bounds the wait; past it the page shows the file as it is.
    let _ = PREPARED_CHANGED.wait_timeout_while(ready, Duration::from_secs(60), |ready| !*ready);
}

/// Successful boss connections confirm only their own CLI; existing selections stay intact.
/// `used` is whether AgentNave ran here before this connection, judged by the caller from the
/// hosts as they were before it.
pub fn confirm_host(root: &Path, id: &str, used: bool) -> Result<(), String> {
    if crate::PREVIEW {
        return Ok(());
    }
    let boss = crate::hosts::boss_of(id);
    let provider = if boss == "workbuddy" {
        "codebuddy"
    } else {
        boss
    };
    // A connection that beats the start-up migration migrates first, so it adds this CLI to
    // what was usable rather than replacing it.
    let open = match read(root)? {
        None => used,
        Some(document) => MODES
            .iter()
            .any(|mode| document["modes"].get(mode).is_none()),
    };
    if open {
        migrate(root, &crate::detect::detected(false), used)?;
    }
    let mut document = read(root)?.unwrap_or_else(empty);
    // Even a boss that is no employee (Hermes) writes the lists, so a later start-up does not
    // take this install for one that ran before.
    let mut changed = confirm_scenes(&mut document, &Value::Null);
    for mode in MODES.iter().filter(|_| PROVIDERS.contains(&provider)) {
        let selection = document["modes"][mode].as_object_mut().unwrap();
        if !selection.contains_key(provider) {
            selection.insert(provider.into(), json!({"models": [], "overrides": {}}));
            changed = true;
        }
    }
    if changed {
        save(root, &document)?;
    }
    Ok(())
}

/// The editable document and why it cannot be saved. Without the file nothing is written:
/// every CLI keeps its own default model and effort.
fn document_state(root: &Path) -> (Value, Value) {
    match read(root) {
        // Without the file no CLI is confirmed yet: both scenes start with an empty list.
        Ok(document) => (
            document.unwrap_or_else(|| {
                let mut document = empty();
                document["modes"] = json!({"task": {}, "discussion": {}});
                document
            }),
            Value::Null,
        ),
        Err(error) => (empty(), json!(error)),
    }
}

/// `GET /settings/members`: the document, why it cannot be edited, and the detected CLIs.
pub fn state(root: &Path, refresh: bool) -> Value {
    wait_prepared();
    let detected = crate::detect::detected(refresh);
    let (document, error) = document_state(root);
    let (review, review_error) = match read_review(root) {
        Ok(review) => (review, Value::Null),
        Err(error) => (Value::Null, json!(error)),
    };
    // Null until the user first picks a tier; every CLI then runs "auto".
    let (permissions, permissions_error) = match read_permissions(root) {
        Ok(permissions) => (permissions.unwrap_or(Value::Null), Value::Null),
        Err(error) => (Value::Null, json!(error)),
    };
    json!({
        "document": document,
        "error": error,
        "review": review,
        "review_error": review_error,
        "permissions": permissions,
        "permissions_error": permissions_error,
        "detected": detected,
    })
}

fn validate_clis(clis: &Map<String, Value>) -> Result<(), String> {
    for (provider, cli) in clis {
        if !PROVIDERS.contains(&provider.as_str()) {
            return Err(format!("不支持的 CLI：{provider}"));
        }
        let fields = object(cli, &format!("clis.{provider}"))?;
        only(fields, &["models"], &format!("clis.{provider}"))?;
        let models = fields
            .get("models")
            .ok_or_else(|| format!("clis.{provider} 缺少 models"))?;
        for (model, shared) in object(models, &format!("clis.{provider}.models"))? {
            if model.trim().is_empty() {
                return Err(format!("clis.{provider} 的模型 ID 不能为空"));
            }
            let what = format!("{provider} / {model}");
            settings(shared, &what)?;
            effort_allowed(shared, None, &what)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
