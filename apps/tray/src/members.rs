//! The user's `members.json`: which model + CLI pairs each mode may dispatch, and their
//! defaults. The tray is its only writer; Python servers read it on every call. Checks here are
//! structural and referential only; Python Adapters validate option semantics.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::Mutex;

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
const FILE: &str = "members.json";
const LEGACY: &str = "profiles.json";
/// Permission-type option keys (case-insensitive substrings) never carried over from profiles.
const PERMISSION_KEYS: [&str; 6] = [
    "dangerously",
    "permission",
    "sandbox",
    "yolo",
    "skip_permissions",
    "bypass",
];
/// Serializes the read-then-migrate step of concurrent `GET /settings/members` requests.
static MIGRATION: Mutex<()> = Mutex::new(());

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
    only(top, &["version", "clis", "modes"], "members.json")?;
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
fn write_atomic(root: &Path, name: &str, document: &Value) -> Result<(), String> {
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

/// Turn each legacy orchestration profile into a shared model configuration. Modes stay absent,
/// so every mode remains unrestricted. Profiles without a model, profiles repeating a CLI + model
/// pair, and permission-type options are skipped and reported.
fn convert(legacy: &Value) -> (Value, Vec<String>) {
    let mut document = empty();
    let mut skipped = Vec::new();
    let mut enabled: Vec<(String, String)> = Vec::new();
    let profiles = legacy["profiles"].as_array().cloned().unwrap_or_default();
    for profile in &profiles {
        let label = profile["name"]
            .as_str()
            .or(profile["id"].as_str())
            .unwrap_or("未命名档案");
        let provider = profile["provider"].as_str().unwrap_or_default();
        if !PROVIDERS.contains(&provider) {
            skipped.push(format!("{label}：不支持的 CLI"));
            continue;
        }
        let mut options = profile["provider_options"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let Some(model) = options
            .remove("model")
            .and_then(|model| model.as_str().map(str::to_owned))
            .filter(|model| !model.trim().is_empty())
        else {
            skipped.push(format!("{label}：未指定模型"));
            continue;
        };
        if enabled.contains(&(provider.to_owned(), model.clone())) {
            skipped.push(format!("{label}：与前面的档案重复（{provider} / {model}）"));
            continue;
        }
        let mut shared = Map::new();
        if let Some(effort) = options.remove("effort")
            && let Some(effort) = effort.as_str().filter(|s| !s.trim().is_empty())
        {
            shared.insert("effort".into(), json!(effort));
        }
        options.retain(|key, value| {
            let lowered = key.to_lowercase();
            if PERMISSION_KEYS.iter().any(|word| lowered.contains(word)) {
                skipped.push(format!("{label}：未迁移权限类参数 {key}"));
                return false;
            }
            value.is_string() || value.is_number() || value.is_boolean()
        });
        if !options.is_empty() {
            shared.insert("options".into(), Value::Object(options));
        }
        if let Some(notes) = profile["notes"].as_str().filter(|s| !s.is_empty()) {
            shared.insert("notes".into(), json!(notes));
        }
        document["clis"][provider]["models"][&model] = Value::Object(shared);
        enabled.push((provider.to_owned(), model));
    }
    (document, skipped)
}

/// First read without `members.json`: migrate `profiles.json`, then rename it out of the way.
/// Returns the new document and the skipped profiles, or `None` when there is nothing to migrate.
pub fn migrate(root: &Path) -> Option<(Value, Vec<String>)> {
    let content = fs::read(root.join(LEGACY)).ok()?;
    let legacy: Value = match serde_json::from_slice(&content) {
        Ok(legacy) => legacy,
        Err(error) => return Some((empty(), vec![format!("{LEGACY} 无法读取，未迁移：{error}")])),
    };
    let (document, skipped) = convert(&legacy);
    if let Err(error) = validate(&document).and_then(|()| write_atomic(root, FILE, &document)) {
        return Some((empty(), vec![format!("迁移 {LEGACY} 失败：{error}")]));
    }
    if let Err(error) = fs::rename(root.join(LEGACY), root.join(format!("{LEGACY}.migrated"))) {
        let mut skipped = skipped;
        skipped.push(format!("已迁移，但无法改名 {LEGACY}：{error}"));
        return Some((document, skipped));
    }
    Some((document, skipped))
}

/// The editable document, why it cannot be saved, and the outcome of a first-read migration.
fn document_state(root: &Path) -> (Value, Value, bool, Vec<String>) {
    let _migrating = MIGRATION
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    match read(root) {
        Ok(Some(document)) => (document, Value::Null, false, Vec::new()),
        Err(error) => (empty(), json!(error), false, Vec::new()),
        Ok(None) => match migrate(root) {
            // A legacy file that could not be migrated stays for the user to fix or remove.
            Some((document, skipped)) => {
                let migrated = root.join(FILE).exists();
                (document, Value::Null, migrated, skipped)
            }
            None => (empty(), Value::Null, false, Vec::new()),
        },
    }
}

/// `GET /settings/members`: the document, why it cannot be edited, and the detected CLIs.
pub fn state(root: &Path, refresh: bool) -> Value {
    let (document, error, migrated, skipped) = document_state(root);
    json!({
        "document": document,
        "error": error,
        "migrated": migrated,
        "migration_skipped": skipped,
        "detected": crate::detect::detected(refresh),
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
    fn broken_file_is_reported_and_left_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_eq!(document_state(root), (empty(), Value::Null, false, vec![]));
        fs::write(root.join(FILE), b"{").unwrap();
        fs::write(root.join(LEGACY), br#"{"version":1,"profiles":[]}"#).unwrap();
        let (document, error, migrated, _) = document_state(root);
        assert_eq!(document, empty());
        assert!(error.as_str().unwrap().contains("损坏") && !migrated);
        assert_eq!(fs::read(root.join(FILE)).unwrap(), b"{");
        assert!(root.join(LEGACY).exists());
    }

    #[test]
    fn migrates_profiles_once_and_reports_skipped_entries() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(migrate(root).is_none());
        let legacy = json!({"version": 1, "profiles": [
            {"id": "a", "name": "核验", "provider": "codex",
             "provider_options": {"model": "gpt-6.1-sol", "effort": "high", "skip_git_repo_check": true,
                                  "Dangerously_Bypass_Approvals_And_Sandbox": true, "permission_mode": "plan"},
             "notes": "只读核验"},
            {"id": "b", "name": "无模型", "provider": "grok", "provider_options": {"effort": "high"}, "notes": ""},
            {"id": "c", "name": "重复", "provider": "codex", "provider_options": {"model": "gpt-6.1-sol"}, "notes": ""},
            {"id": "d", "name": "讨论", "provider": "pi", "provider_options": {"model": "magpie/x"}, "notes": ""}
        ]});
        fs::write(root.join(LEGACY), legacy.to_string()).unwrap();

        let (document, _, migrated, skipped) = document_state(root);
        assert!(migrated);
        assert_eq!(
            document["clis"]["codex"]["models"]["gpt-6.1-sol"],
            json!({"effort": "high", "options": {"skip_git_repo_check": true}, "notes": "只读核验"})
        );
        assert_eq!(document["clis"]["pi"]["models"]["magpie/x"], json!({}));
        // Migration never restricts a mode: behavior stays as before.
        assert_eq!(document["modes"], json!({}));
        assert_eq!(skipped.len(), 4);
        assert!(skipped[0].contains("Dangerously_Bypass_Approvals_And_Sandbox"));
        assert!(skipped[1].contains("permission_mode"));
        assert!(skipped[2].starts_with("无模型") && skipped[3].starts_with("重复"));
        assert_eq!(read(root).unwrap(), Some(document));
        assert!(!root.join(LEGACY).exists());
        assert!(root.join("profiles.json.migrated").exists());
        assert!(migrate(root).is_none());
    }

    #[test]
    fn migration_without_any_model_keeps_modes_unrestricted() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let legacy = json!({"version": 1, "profiles": [
            {"id": "b", "name": "无模型", "provider": "grok", "provider_options": {}, "notes": ""}
        ]});
        fs::write(root.join(LEGACY), legacy.to_string()).unwrap();
        let (document, skipped) = migrate(root).unwrap();
        assert_eq!(document, empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(read(root).unwrap(), Some(empty()));
    }
}
