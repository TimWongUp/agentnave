//! The user's `members.json`: which model + CLI pairs each mode may dispatch, and their
//! defaults. The tray is its only writer; Python servers read it on every call. Checks here are
//! structural and referential only; Python Adapters validate option semantics.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use serde_json::{Map, Value, json};

pub const PROVIDERS: [&str; 7] = [
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
pub const REVIEW_ROLES: [&str; 8] = [
    "correctness-reviewer",
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
        let what = format!("seats.{role}");
        let fields = object(seat, &what)?;
        only(fields, &["provider", "model", "effort"], &what)?;
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
    }
    Ok(())
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

/// Run at App start: probe the CLIs once so the office can name each CLI's own default model
/// before Settings is ever opened.
pub fn prepare() {
    crate::detect::detected(false);
}

/// The editable document and why it cannot be saved. Without the file nothing is written:
/// every CLI keeps its own default model and effort.
fn document_state(root: &Path) -> (Value, Value) {
    match read(root) {
        Ok(document) => (document.unwrap_or_else(empty), Value::Null),
        Err(error) => (empty(), json!(error)),
    }
}

/// `GET /settings/members`: the document, why it cannot be edited, and the detected CLIs.
pub fn state(root: &Path, refresh: bool) -> Value {
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

#[cfg(test)]
mod tests {

    use super::*;
    use crate::data::Reader;

    fn document() -> Value {
        json!({
            "version": 1,
            "clis": {"codex": {"models": {
                "gpt-6.1-sol": {
                    "efforts": ["medium", "high", "xhigh"],
                    "effort": "high",
                    "options": {"skip_git_repo_check": true, "budget": 2.5},
                    "notes": "多文件实现"
                },
                "gpt-6-luna": {}
            }}},
            "modes": {
                "task": {"codex": {
                    "models": ["gpt-6.1-sol"],
                    "default": "gpt-6.1-sol",
                    "overrides": {"gpt-6.1-sol": {"effort": "medium"}}
                }},
                "discussion": {}
            }
        })
    }

    #[test]
    fn accepts_the_contract_example_and_rejects_structural_or_reference_errors() {
        validate(&document()).unwrap();
        validate(&empty()).unwrap();
        let mut deepseek = document();
        deepseek["clis"]["deepseek"] = json!({"models": {"deepseek-flash": {"effort": "max"}}});
        deepseek["modes"]["task"]["deepseek"] = json!({"models": ["deepseek-flash"]});
        validate(&deepseek).unwrap();
        // Review seats live in review.json and may name models no longer configured.
        let mut members_with_review = document();
        members_with_review["review"] = json!({});
        assert!(validate(&members_with_review).is_err());
        let mut review = json!({"version": 1, "seats": {
            "correctness-reviewer": {"provider": "deepseek", "model": "gone", "effort": "max"},
            "security-reviewer": {"provider": "codex", "model": "gpt-6.1-sol"}
        }});
        validate_review(&review).unwrap();
        for seat in [
            json!({"other-reviewer": {"provider": "codex", "model": "m"}}),
            json!({"security-reviewer": {"provider": "other", "model": "m"}}),
            json!({"security-reviewer": {"provider": "codex"}}),
            json!({"security-reviewer": {"provider": "codex", "model": " "}}),
            json!({"security-reviewer": {"provider": "codex", "model": "m", "effort": 1}}),
            json!({"security-reviewer": {"provider": "codex", "model": "m", "extra": 1}}),
        ] {
            review["seats"] = seat.clone();
            assert!(validate_review(&review).is_err(), "{seat}");
        }
        for key in ["version", "clis", "modes"] {
            let mut missing = empty();
            missing.as_object_mut().unwrap().remove(key);
            assert!(validate(&missing).is_err(), "{key}");
        }
        for pointer in ["/clis/codex", "/modes/task/codex"] {
            let mut missing = document();
            let entry = missing.pointer_mut(pointer).unwrap();
            entry.as_object_mut().unwrap().remove("models");
            assert!(validate(&missing).is_err(), "{pointer}");
        }
        let cases: Vec<(&str, Value)> = vec![
            ("/extra", json!(true)),
            ("/version", json!(2)),
            ("/clis/other", json!({"models": {}})),
            ("/clis/codex/extra", json!({})),
            ("/clis/codex/models/gpt-6-luna/unknown", json!(1)),
            (
                "/clis/codex/models/gpt-6-luna/options",
                json!({"model": "x"}),
            ),
            ("/clis/codex/models/gpt-6-luna/options", json!({"x": []})),
            ("/clis/codex/models/gpt-6.1-sol/effort", json!("low")),
            ("/clis/codex/models/gpt-6.1-sol/efforts", json!(["high", 1])),
            ("/modes/review", json!({})),
            ("/modes/discussion/grok", json!({"models": ["grok-4.7"]})),
            (
                "/modes/task/codex/models",
                json!(["gpt-6.1-sol", "missing"]),
            ),
            ("/modes/task/codex/default", json!("gpt-6-luna")),
            (
                "/modes/task/codex/overrides",
                json!({"gpt-6.1-sol": {"effort": "low"}}),
            ),
            ("/modes/task/codex/overrides", json!({"missing": {}})),
            ("/clis/codex/models/ ", json!({})),
            ("/modes/task/codex/extra", json!(1)),
        ];
        for (pointer, value) in cases {
            let mut bad = document();
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            bad.pointer_mut(parent).unwrap()[key] = value;
            assert!(validate(&bad).is_err(), "{pointer}");
        }
        // An override may replace the allowed efforts so its default fits again.
        let mut widened = document();
        widened["modes"]["task"]["codex"]["overrides"]["gpt-6.1-sol"] =
            json!({"efforts": ["low"], "effort": "low"});
        validate(&widened).unwrap();
        // An override only needs a configured model, as on the server; duplicates are tolerated.
        let mut lenient = document();
        lenient["modes"]["task"]["codex"]["overrides"]["gpt-6-luna"] = json!({"effort": ""});
        lenient["modes"]["task"]["codex"]["models"] = json!(["gpt-6.1-sol", "gpt-6.1-sol"]);
        validate(&lenient).unwrap();
        // With no listed models, the default only has to be configured.
        let mut open = document();
        open["modes"]["task"]["codex"] = json!({"models": [], "default": "gpt-6-luna"});
        validate(&open).unwrap();
        // A CLI enabled with any model needs no shared model configuration.
        open["modes"]["discussion"]["grok"] = json!({"models": []});
        validate(&open).unwrap();
        // An unrestricted mode keeps its default models under `defaults`.
        let mut kept = document();
        kept["modes"].as_object_mut().unwrap().remove("discussion");
        kept["defaults"] = json!({"discussion": {"codex": "gpt-6-luna"}});
        validate(&kept).unwrap();
        for (mode, model) in [("discussion", "missing"), ("task", "gpt-6-luna")] {
            let mut bad = kept.clone();
            bad["defaults"] = json!({mode: {"codex": model}});
            assert!(validate(&bad).is_err(), "{mode}");
        }
    }

    #[test]
    fn saves_atomically_through_the_post_route_only() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let reader = Reader::new(root.clone());
        let body = serde_json::to_vec(&document()).unwrap();
        assert_eq!(
            reader
                .request("GET", "/app/members", Some("1"), &body)
                .status,
            405
        );
        assert_eq!(
            reader.request("POST", "/app/members", None, &body).status,
            403
        );
        assert_eq!(
            reader
                .request("POST", "/app/members", Some("1"), b"{")
                .status,
            400
        );
        let mut bad = document();
        bad["modes"]["task"]["codex"]["default"] = json!("missing");
        let response = reader.request(
            "POST",
            "/app/members",
            Some("1"),
            &serde_json::to_vec(&bad).unwrap(),
        );
        assert_eq!(response.status, 400);
        assert!(!root.exists());
        assert_eq!(
            reader
                .request("POST", "/app/members", Some("1"), &body)
                .status,
            200
        );
        assert_eq!(read(&root).unwrap(), Some(document()));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        // Rename cannot replace a directory; the old destination and no temporary file remain.
        fs::remove_file(root.join(FILE)).unwrap();
        fs::create_dir(root.join(FILE)).unwrap();
        assert!(save(&root, &document()).is_err());
        assert!(root.join(FILE).is_dir());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn review_seats_save_to_their_own_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let reader = Reader::new(root.clone());
        assert_eq!(read_review(&root).unwrap()["seats"], json!({}));
        let seats = json!({"version": 1, "seats": {
            "correctness-reviewer": {"provider": "codex", "model": "gpt-6.1-sol"}
        }});
        let post = |body: &Value| {
            reader
                .request(
                    "POST",
                    "/app/review",
                    Some("1"),
                    &serde_json::to_vec(body).unwrap(),
                )
                .status
        };
        assert_eq!(post(&json!({"version": 1, "seats": {"x": {}}})), 400);
        assert_eq!(post(&seats), 200);
        assert_eq!(read_review(&root).unwrap(), seats);
        assert!(!root.join(FILE).exists());
        fs::write(root.join(REVIEW_FILE), b"{").unwrap();
        assert!(read_review(&root).is_err());
    }

    #[test]
    fn permission_tiers_save_to_their_own_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let reader = Reader::new(root.clone());
        assert_eq!(read_permissions(&root).unwrap(), None);
        let post = |body: &Value| {
            reader
                .request(
                    "POST",
                    "/app/permissions",
                    Some("1"),
                    &serde_json::to_vec(body).unwrap(),
                )
                .status
        };
        for clis in [
            json!({"pi": "auto"}),
            json!({"other": "auto"}),
            json!({"codex": "yolo"}),
            json!({"codex": "native"}),
            json!({"codex": true}),
        ] {
            assert_eq!(post(&json!({"version": 1, "clis": clis})), 400);
        }
        let tiers = json!({"version": 1, "clis": {"codex": "auto", "claude": "full"}});
        assert_eq!(post(&tiers), 200);
        assert_eq!(read_permissions(&root).unwrap(), Some(tiers));
        assert!(!root.join(FILE).exists());
        fs::write(root.join(PERMISSIONS_FILE), b"{").unwrap();
        assert!(read_permissions(&root).is_err());
    }

    #[test]
    fn broken_file_is_reported_and_left_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_eq!(document_state(root), (empty(), Value::Null));
        fs::write(root.join(FILE), b"{").unwrap();
        let (document, error) = document_state(root);
        assert_eq!(document, empty());
        assert!(error.as_str().unwrap().contains("损坏"));
        assert_eq!(fs::read(root.join(FILE)).unwrap(), b"{");
    }

    #[test]
    fn a_missing_document_stays_missing_so_every_cli_keeps_its_own_default() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_eq!(document_state(root), (empty(), Value::Null));
        assert!(!root.join(FILE).exists());
    }
}
