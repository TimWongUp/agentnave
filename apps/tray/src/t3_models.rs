//! Read-only probe of T3 Code's provider caches and hidden-model preferences (ADR-0121).
//!
//! Privacy boundary: only `instanceId`, `driver`, `displayName`, `status`, `installed`, `enabled`,
//! `auth.status`, `checkedAt`, `models[].{slug, name, aliases, isLegacy, isDefault,
//! capabilities.optionDescriptors}` and `providerModelPreferences.<instanceId>.hiddenModels` are
//! read. Other `auth` fields (such as emails or workspace IDs) are never read, stored or logged.
//! T3 Code files are never written.

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Hard cap per T3 cache or settings file so oversized or corrupt files are skipped safely.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Instances whose `checkedAt` trails the newest instance by more than 24 h are marked stale.
const STALE_AFTER_SECS: i64 = 24 * 3600;

struct ParsedInstance {
    instance: String,
    driver: String,
    name: String,
    available: bool,
    checked_at: Option<i64>,
    models: Vec<Value>,
}

/// T3 Code data root: `T3CODE_HOME` if non-empty, else `~/.t3`.
pub fn resolve_home(
    env: Option<&BTreeMap<String, String>>,
    user_home: Option<&Path>,
) -> Option<PathBuf> {
    let from_env = match env {
        Some(vars) => vars
            .get("T3CODE_HOME")
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty()),
        None => std::env::var_os("T3CODE_HOME")
            .map(|value| value.to_string_lossy().trim().to_owned())
            .filter(|value| !value.is_empty()),
    };
    if let Some(dir) = from_env {
        return Some(PathBuf::from(dir));
    }
    user_home
        .map(Path::to_path_buf)
        .or_else(dirs::home_dir)
        .map(|home| home.join(".t3"))
}

fn read_bounded_json(path: &Path) -> Option<Value> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let file = File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(meta.len().min(65536) as usize);
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn read_hidden_models(t3_home: &Path) -> HashMap<String, HashSet<String>> {
    let mut out = HashMap::new();
    let Some(root) = read_bounded_json(&t3_home.join("userdata").join("settings.json")) else {
        return out;
    };
    let Some(prefs) = root
        .get("providerModelPreferences")
        .and_then(Value::as_object)
    else {
        return out;
    };
    for (instance_id, entry) in prefs {
        let Some(hidden) = entry.get("hiddenModels").and_then(Value::as_array) else {
            continue;
        };
        let models: HashSet<String> = hidden
            .iter()
            .filter_map(Value::as_str)
            .filter(|slug| !slug.trim().is_empty())
            .map(str::to_owned)
            .collect();
        if !models.is_empty() {
            out.insert(instance_id.clone(), models);
        }
    }
    out
}

fn parse_checked_at(raw: Option<&Value>) -> Option<i64> {
    match raw? {
        Value::String(text) => chrono::DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|moment| moment.timestamp()),
        Value::Number(number) => {
            let value = number.as_f64()?;
            if !value.is_finite() || value <= 0.0 {
                return None;
            }
            let seconds = if value > 1e11 { value / 1000.0 } else { value };
            Some(seconds as i64)
        }
        _ => None,
    }
}

fn descriptor_to_reasoning(descriptor: &Map<String, Value>) -> Option<Value> {
    if descriptor.get("type").and_then(Value::as_str) != Some("select") {
        return None;
    }
    let id = descriptor
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && *id == id.trim())?;
    if crate::t3_seats::is_forbidden_option_id(id) {
        return None;
    }
    let raw_options = descriptor.get("options")?.as_array()?;
    let mut options = Vec::new();
    for item in raw_options {
        let Some(fields) = item.as_object() else {
            continue;
        };
        // T3 caches store choice identifiers under `id`; accept `value` as a fallback too.
        let Some(value) = fields
            .get("id")
            .or_else(|| fields.get("value"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let label = fields
            .get("label")
            .and_then(Value::as_str)
            .filter(|label| !label.trim().is_empty())
            .unwrap_or(value);
        options.push(json!({"value": value, "label": label}));
    }
    if options.is_empty() {
        return None;
    }
    let label = descriptor
        .get("label")
        .and_then(Value::as_str)
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(id);
    Some(json!({"id": id, "label": label, "options": options}))
}

fn extract_reasoning(model: &Map<String, Value>) -> Option<Value> {
    let descriptors = model
        .get("capabilities")?
        .as_object()?
        .get("optionDescriptors")?
        .as_array()?;
    let objects: Vec<&Map<String, Value>> =
        descriptors.iter().filter_map(Value::as_object).collect();
    // Prefer descriptors explicitly marked with role == "reasoning", then fall back to known IDs.
    let explicit = objects
        .iter()
        .filter(|desc| desc.get("role").and_then(Value::as_str) == Some("reasoning"))
        .find_map(|desc| descriptor_to_reasoning(desc));
    if explicit.is_some() {
        return explicit;
    }
    objects
        .iter()
        .filter(|desc| {
            matches!(
                desc.get("id").and_then(Value::as_str),
                Some("effort" | "reasoningEffort")
            )
        })
        .find_map(|desc| descriptor_to_reasoning(desc))
}

fn extract_model(raw: &Value, hidden: Option<&HashSet<String>>) -> Option<Value> {
    let fields = raw.as_object()?;
    let slug = fields
        .get("slug")
        .and_then(Value::as_str)
        .filter(|slug| !slug.is_empty() && *slug == slug.trim())?;
    let name = fields
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(slug);
    let aliases: Vec<&str> = fields
        .get("aliases")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .filter(|alias| !alias.trim().is_empty())
                .collect()
        })
        .unwrap_or_default();
    let legacy = fields
        .get("isLegacy")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let default = fields
        .get("isDefault")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let is_hidden = hidden.is_some_and(|set| set.contains(slug));
    let reasoning = extract_reasoning(fields);
    Some(json!({
        "slug": slug,
        "name": name,
        "aliases": aliases,
        "legacy": legacy,
        "hidden": is_hidden,
        "default": default,
        "reasoning": reasoning,
    }))
}

fn parse_instance(
    raw: &Value,
    hidden_map: &HashMap<String, HashSet<String>>,
) -> Option<ParsedInstance> {
    let fields = raw.as_object()?;
    let raw_models = fields.get("models")?.as_array()?;
    let instance = fields
        .get("instanceId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && *id == id.trim())?;
    let driver = fields
        .get("driver")
        .and_then(Value::as_str)
        .filter(|driver| !driver.is_empty() && *driver == driver.trim())?;
    if fields.get("enabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let name = fields
        .get("displayName")
        .and_then(Value::as_str)
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(instance);
    let status = fields.get("status").and_then(Value::as_str);
    let installed = fields.get("installed").and_then(Value::as_bool);
    let auth_status = fields
        .get("auth")
        .and_then(Value::as_object)
        .and_then(|auth| auth.get("status"))
        .and_then(Value::as_str);
    let available = !matches!(status, Some("error" | "disabled"))
        && auth_status != Some("unauthenticated")
        && installed != Some(false);
    let checked_at = parse_checked_at(fields.get("checkedAt"));
    let hidden = hidden_map.get(instance);
    let models = raw_models
        .iter()
        .filter_map(|model| extract_model(model, hidden))
        .collect();
    Some(ParsedInstance {
        instance: instance.to_owned(),
        driver: driver.to_owned(),
        name: name.to_owned(),
        available,
        checked_at,
        models,
    })
}

/// Reads T3 Code instances and models from `t3_home` without writing anything.
pub fn read_from(t3_home: &Path) -> Value {
    let hidden_map = read_hidden_models(t3_home);
    let mut entries: Vec<PathBuf> = fs::read_dir(t3_home.join("caches"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    entries.sort();

    let mut by_id: HashMap<String, ParsedInstance> = HashMap::new();
    for path in entries {
        let Some(raw) = read_bounded_json(&path) else {
            continue;
        };
        let Some(parsed) = parse_instance(&raw, &hidden_map) else {
            continue;
        };
        let replace = by_id
            .get(&parsed.instance)
            .is_none_or(|old| parsed.checked_at > old.checked_at);
        if replace {
            by_id.insert(parsed.instance.clone(), parsed);
        }
    }

    let latest = by_id.values().filter_map(|item| item.checked_at).max();
    let mut items: Vec<(bool, ParsedInstance)> = by_id
        .into_values()
        .map(|item| {
            let stale = match (latest, item.checked_at) {
                (Some(max_ts), Some(ts)) => max_ts - ts > STALE_AFTER_SECS,
                _ => false,
            };
            (stale, item)
        })
        .collect();
    items.sort_by(|(stale_a, a), (stale_b, b)| {
        (
            *stale_a,
            !a.available,
            a.driver.to_lowercase(),
            a.name.to_lowercase(),
            a.instance.to_lowercase(),
        )
            .cmp(&(
                *stale_b,
                !b.available,
                b.driver.to_lowercase(),
                b.name.to_lowercase(),
                b.instance.to_lowercase(),
            ))
    });

    let detected = !items.is_empty() || t3_home.join("userdata").join("settings.json").is_file();
    let instances: Vec<Value> = items
        .into_iter()
        .map(|(stale, item)| {
            json!({
                "instance": item.instance,
                "driver": item.driver,
                "name": item.name,
                "available": item.available,
                "stale": stale,
                "models": item.models,
            })
        })
        .collect();
    json!({
        "detected": detected,
        "instances": instances,
    })
}

/// Reads T3 Code instances using `env` + `user_home` when given, or the live environment.
pub fn read_for(env: Option<&BTreeMap<String, String>>, user_home: Option<&Path>) -> Value {
    match resolve_home(env, user_home) {
        Some(home) => read_from(&home),
        None => json!({"detected": false, "instances": []}),
    }
}

#[cfg(test)]
mod tests;
