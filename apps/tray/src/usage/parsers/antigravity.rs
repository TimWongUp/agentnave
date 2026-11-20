use super::{event, number, string, timestamp};
use crate::usage::Event;
use serde_json::Value;
use std::path::Path;

pub(super) fn parse(path: &Path, v: &Value) -> Option<Event> {
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
    e.fresh = number(v, "input_tokens");
    e.read = number(v, "cache_read_tokens");
    e.output = number(v, "output_tokens");
    e.unknown = v.get("cache_read_tokens").is_none();
    Some(e)
}
