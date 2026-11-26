use super::{event, number, string, timestamp};
use crate::usage::Event;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

pub(super) fn parse(
    path: &Path,
    v: &Value,
    cwds: Option<&HashMap<String, String>>,
) -> Option<Event> {
    if string(v, "source") != "MODEL" {
        return None;
    }
    let id = path.parent()?.parent()?.parent()?.file_name()?.to_str()?;
    let step = v["step_index"].as_i64()?;
    let mut e = event(
        "antigravity",
        id,
        id,
        timestamp(v, "created_at")?,
        string(v, "model"),
        format!("agy:{id}:{step}"),
    );
    if let Some(cwd) = cwds.and_then(|map| map.get(id)) {
        e.cwd = cwd.clone();
    }
    e.fresh = number(v, "input_tokens");
    e.read = number(v, "cache_read_tokens");
    e.output = number(v, "output_tokens");
    if e.total() == 0 {
        return None;
    }
    e.unknown = v.get("cache_read_tokens").is_none();
    Some(e)
}

/// Batch-load `conversation_id -> first file:// workspace path` once per Antigravity root
/// from its read-only `conversation_summaries.db`.
pub(crate) fn load_cwds(root: &Path) -> HashMap<String, String> {
    let db_path = root.join("conversation_summaries.db");
    if !db_path.is_file() {
        return HashMap::new();
    }
    let Ok(conn) = crate::usage::zcode::open_with(&db_path, Duration::from_millis(500)) else {
        return HashMap::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT conversation_id, workspace_uris FROM conversation_summaries WHERE workspace_uris != ''",
    ) else {
        return HashMap::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for (id, uris) in rows.flatten() {
        if !id.is_empty()
            && let Some(cwd) = first_workspace_cwd(&uris)
        {
            out.insert(id, cwd);
        }
    }
    out
}

fn first_workspace_cwd(workspace_uris: &str) -> Option<String> {
    let Value::Array(uris) = serde_json::from_str::<Value>(workspace_uris).ok()? else {
        return None;
    };
    uris.iter()
        .filter_map(Value::as_str)
        .find_map(decode_file_uri)
}

fn decode_file_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let (host, raw_path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, ""),
    };
    if cfg!(windows) {
        let host_dec = crate::usage::percent_decode(host)?;
        let decoded = crate::usage::percent_decode(raw_path)?;
        if host_dec.len() == 2
            && host_dec.as_bytes()[0].is_ascii_alphabetic()
            && host_dec.as_bytes()[1] == b':'
        {
            let drive = (host_dec.as_bytes()[0] as char).to_ascii_uppercase();
            return Some(if decoded.is_empty() || decoded == "/" {
                format!("{drive}:\\")
            } else {
                format!("{drive}:{}", decoded.replace('/', "\\"))
            });
        }
        if !host_dec.is_empty() && !host_dec.eq_ignore_ascii_case("localhost") {
            return (!decoded.is_empty())
                .then(|| format!("\\\\{host_dec}{}", decoded.replace('/', "\\")));
        }
        if let Some(after) = decoded.strip_prefix('/')
            && after.len() >= 2
            && after.as_bytes()[0].is_ascii_alphabetic()
            && after.as_bytes()[1] == b':'
        {
            let drive = (after.as_bytes()[0] as char).to_ascii_uppercase();
            return Some(format!("{drive}:{}", after[2..].replace('/', "\\")));
        }
        return (!decoded.is_empty()).then(|| decoded.replace('/', "\\"));
    }
    if raw_path.is_empty() || (!host.is_empty() && !host.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    let decoded = crate::usage::percent_decode(raw_path)?;
    (!decoded.is_empty()).then_some(decoded)
}
