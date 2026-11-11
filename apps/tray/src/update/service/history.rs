//! Bounded evidence of service switches, used only for idle host guidance.
use super::*;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

const FILE: &str = "install-history.json";
const LIMIT: u64 = 64 * 1024;
const KEEP: usize = 10;

fn read(base: &Path) -> Option<Vec<Value>> {
    let path = base.join(FILE);
    if fs::metadata(&path).ok()?.len() > LIMIT {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn names(env: &Path) -> Option<Value> {
    let site = installed_metadata(env)?.parent()?.to_path_buf();
    let path = site.join("agentnave/tool_names.json");
    if fs::metadata(&path).ok()?.len() > LIMIT {
        return None;
    }
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    valid_names(&value).then_some(value)
}

fn valid_names(value: &Value) -> bool {
    let Some(list) = value.as_array() else {
        return false;
    };
    !(list.is_empty()
        || list.iter().any(|v| v.as_str().is_none_or(str::is_empty))
        || list.windows(2).any(|w| w[0].as_str() >= w[1].as_str()))
}

pub(super) fn record(
    base: &Path,
    version: &str,
    previous: Option<&str>,
    env: &Path,
    at: SystemTime,
) {
    let now = at.duration_since(UNIX_EPOCH).ok();
    let Some(now) = now else { return };
    let mut history = read(base).unwrap_or_default();
    if history.last().and_then(|entry| entry["version"].as_str()) != previous {
        history.clear();
    }
    history.push(
        json!({"version":version,"switched_at_ms":now.as_millis() as u64,
        "tool_names":names(env)}),
    );
    if history.len() > KEEP {
        history.drain(..history.len() - KEEP);
    }
    let Ok(bytes) = serde_json::to_vec(&history) else {
        return;
    };
    if bytes.len() as u64 > LIMIT {
        return;
    }
    if let Ok(mut file) = tempfile::NamedTempFile::new_in(base)
        && file.write_all(&bytes).is_ok()
    {
        let _ = file.persist(base.join(FILE));
    }
}

/// Return the latest switch if the names active since process start (or a later confirmed switch)
/// differ from the current ones.
pub(crate) fn tool_change(
    app: &Path,
    version: Option<&str>,
    started: u64,
    confirmed: Option<u64>,
) -> Option<u64> {
    let version = version?;
    let history = read(&app.join("service"))?;
    let current = history.last()?;
    let switched = current["switched_at_ms"].as_u64()?;
    if current["version"].as_str() != Some(version)
        || started == 0
        || started >= switched / 1000
        || confirmed.is_some_and(|at| at / 1000 == started)
    {
        return None;
    }
    if history.iter().any(|entry| {
        entry["switched_at_ms"]
            .as_u64()
            .is_none_or(|at| at / 1000 == started)
    }) || history
        .windows(2)
        .any(|pair| pair[0]["switched_at_ms"].as_u64() >= pair[1]["switched_at_ms"].as_u64())
    {
        return None;
    }
    let old = match confirmed.filter(|at| at / 1000 > started) {
        Some(at) => history
            .iter()
            .find(|entry| entry["switched_at_ms"].as_u64() == Some(at))?,
        None => history.iter().rev().find(|entry| {
            entry["switched_at_ms"]
                .as_u64()
                .is_some_and(|at| at / 1000 < started)
        })?,
    };
    let before = &old["tool_names"];
    let after = &current["tool_names"];
    (valid_names(before) && valid_names(after) && before != after).then_some(switched)
}

pub(crate) fn has_tool_history(app: &Path) -> bool {
    read(&app.join("service")).is_some_and(|history| history.len() >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_installed_names_and_unknown_with_bounded_history() {
        let temp = tempfile::tempdir().unwrap();
        let env = temp.path().join("env");
        let site = env.join("lib/python3.12/site-packages");
        fs::create_dir_all(site.join("agentnave-1.dist-info")).unwrap();
        fs::create_dir_all(site.join("agentnave")).unwrap();
        fs::write(site.join("agentnave/tool_names.json"), "[\"a\",\"b\"]").unwrap();
        record(temp.path(), "1", None, &env, SystemTime::now());
        assert_eq!(
            read(temp.path()).unwrap()[0]["tool_names"],
            json!(["a", "b"])
        );
        fs::remove_file(site.join("agentnave/tool_names.json")).unwrap();
        record(temp.path(), "2", Some("1"), &env, SystemTime::now());
        for _ in 0..9 {
            record(temp.path(), "2", Some("2"), &env, SystemTime::now());
        }
        let history = read(temp.path()).unwrap();
        assert_eq!(history.len(), 10);
        assert_eq!(history[9]["version"], "2");
        assert!(history[9]["tool_names"].is_null());
        record(temp.path(), "4", Some("3"), &env, SystemTime::now());
        assert_eq!(read(temp.path()).unwrap().len(), 1);
    }
}
