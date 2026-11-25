//! The fixed role catalog the server, the App and the generated rules share, plus shared seat
//! metadata checks mirrored from `src/agentnave/orchestration_roles.py`.
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

pub(crate) const MAX_SEATS: usize = 255;
pub(crate) const ROLE_MAX_CHARS: usize = 40;
pub(crate) const DESCRIPTION_MAX_CHARS: usize = 200;
/// The kind of a seat whose specialty the user named; any other kind is a group of the role catalog.
pub(crate) const CUSTOM_KIND: &str = "custom";
/// The kind of an image generation seat: write access, no difficulty, one card per seat.
pub(crate) const IMAGE_KIND: &str = "image";
pub(crate) const DIFFICULTIES: [&str; 3] = ["easy", "medium", "hard"];

/// What the text of a seat may not hold, by code point, listed one by one: the service
/// (`_REFUSED_CODES` in orchestration_roles.py) checks the very same table, so no Unicode category
/// table decides. Control characters, white space but the ordinary space, invisible format
/// characters and private use (surrogates cannot occur in a Rust string).
const REFUSED: [(u32, u32); 19] = [
    (0x0000, 0x001F),
    (0x007F, 0x009F),
    (0x00A0, 0x00A0),
    (0x00AD, 0x00AD),
    (0x061C, 0x061C),
    (0x1680, 0x1680),
    (0x180E, 0x180E),
    (0x2000, 0x200F),
    (0x2028, 0x202F),
    (0x205F, 0x205F),
    (0x2060, 0x2064),
    (0x2066, 0x206F),
    (0x3000, 0x3000),
    (0xD800, 0xDFFF),
    (0xE000, 0xF8FF),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0xF0000, 0xFFFFD),
    (0x100000, 0x10FFFD),
];
/// The white space trimmed from both ends first; inside the text only the ordinary space is kept.
const TRIMMED: [(u32, u32); 10] = [
    (0x09, 0x0D),
    (0x20, 0x20),
    (0x85, 0x85),
    (0xA0, 0xA0),
    (0x1680, 0x1680),
    (0x2000, 0x200A),
    (0x2028, 0x2029),
    (0x202F, 0x202F),
    (0x205F, 0x205F),
    (0x3000, 0x3000),
];

static CATALOG: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../src/agentnave/orchestration_roles.json"
    ))
    .expect("valid bundled orchestration roles")
});

/// Every role in display order, as the settings page reads it from GET /app/orchestration.
pub fn catalog() -> &'static Value {
    &CATALOG
}

fn roles() -> &'static [Value] {
    catalog().as_array().map_or(&[], Vec::as_slice)
}

fn group(role: &'static Value) -> Option<&'static str> {
    role["group"].as_str()
}

pub fn groups() -> Vec<&'static str> {
    roles().iter().filter_map(group).collect()
}

/// The read or write type a role-catalog group is fixed to; None for any other kind.
pub(crate) fn group_access(kind: &str) -> Option<&'static str> {
    roles()
        .iter()
        .find(|role| role["group"] == kind)
        .and_then(|role| role["access"].as_str())
}

pub(crate) fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 128
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn listed(table: &[(u32, u32)], c: char) -> bool {
    table
        .iter()
        .any(|&(low, high)| (low..=high).contains(&u32::from(c)))
}

pub(crate) fn trim_text(text: &str) -> &str {
    text.trim_matches(|c| listed(&TRIMMED, c))
}

/// Trimmed text of 1 to `limit` characters the table allows.
pub(crate) fn printable_text(text: Option<&str>, limit: usize) -> bool {
    let text = text.map(trim_text).unwrap_or_default();
    !text.is_empty() && text.chars().count() <= limit && !text.chars().any(|c| listed(&REFUSED, c))
}

pub(crate) fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{what} 必须为对象"))
}

/// Validates the metadata fields shared by `CustomSeat` and `T3Seat` (`SeatMetadata` on the server).
pub(crate) fn validate_seat_metadata(
    fields: &Map<String, Value>,
    what: &str,
) -> Result<(), String> {
    let text = |key: &str| fields.get(key).and_then(Value::as_str);
    if !text("id").is_some_and(valid_id) {
        return Err(format!("{what} 的标识无效"));
    }
    let kind = text("kind").unwrap_or_default();
    let fixed = group_access(kind);
    if ![CUSTOM_KIND, IMAGE_KIND].contains(&kind) && fixed.is_none() {
        return Err(format!("{what} 的类型无效"));
    }
    if !printable_text(text("role"), ROLE_MAX_CHARS) {
        return Err(format!("{what} 的专业需为 1–{ROLE_MAX_CHARS} 个可打印字符"));
    }
    if !printable_text(text("description"), DESCRIPTION_MAX_CHARS) {
        return Err(format!(
            "{what} 的说明需为 1–{DESCRIPTION_MAX_CHARS} 个可打印字符"
        ));
    }
    if !matches!(text("access"), Some("read" | "write")) {
        return Err(format!("{what} 的读写类型必须为 read 或 write"));
    }
    if fixed.is_some_and(|access| Some(access) != text("access")) {
        return Err(format!("{what} 的读写类型与所属工位分组不符"));
    }
    if kind == IMAGE_KIND {
        // A null difficulty is as good as none, as the service reads it.
        if text("access") != Some("write")
            || fields
                .get("difficulty")
                .is_some_and(|value| !value.is_null())
        {
            return Err(format!("{what} 是生图工位，只能可写且没有难度"));
        }
    } else if !text("difficulty").is_some_and(|difficulty| DIFFICULTIES.contains(&difficulty)) {
        return Err(format!("{what} 的难度无效"));
    }
    Ok(())
}

/// Validates unique seat IDs and per-card consistency across a task-seat catalog
/// (`validate_seat_cards` on the server).
pub(crate) fn validate_seat_cards(seats: &[Value]) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut levels = HashSet::new();
    let mut group_roles = HashMap::new();
    let mut card_access = HashMap::new();
    for (index, seat) in seats.iter().enumerate() {
        if !ids.insert(seat["id"].as_str().unwrap_or_default()) {
            return Err(format!("工位标识重复：{}", seat["id"]));
        }
        let (kind, role) = (seat["kind"].as_str(), seat["role"].as_str().map(trim_text));
        if !levels.insert((kind, role, seat["difficulty"].as_str())) {
            return Err(format!("seats[{index}] 与同一张卡片的另一档重复"));
        }
        // A card shows one access, so its seats must agree on it.
        if *card_access
            .entry((kind, role))
            .or_insert(seat["access"].as_str())
            != seat["access"].as_str()
        {
            return Err(format!("seats[{index}] 与同一张卡片的读写类型不一致"));
        }
        if ![Some(CUSTOM_KIND), Some(IMAGE_KIND)].contains(&kind)
            && *group_roles.entry(kind).or_insert(role) != role
        {
            return Err(format!("seats[{index}] 的分组对应了不同的名称"));
        }
    }
    Ok(())
}

/// The groups a document turns on, in catalog order. A missing or null `enabled_roles` keeps the
/// groups that existed before the switches; `[]` turns them all off.
pub fn enabled_groups(document: &Value) -> Result<Vec<&'static str>, String> {
    let picked = match document.get("enabled_roles") {
        None | Some(Value::Null) => {
            let legacy = roles().iter().filter(|role| role["legacy_enabled"] == true);
            return Ok(legacy.filter_map(group).collect());
        }
        Some(Value::Array(picked)) => picked,
        Some(_) => return Err("enabled_roles 必须为数组".into()),
    };
    let known = groups();
    let mut seen = HashSet::new();
    for role in picked {
        let role = role.as_str().ok_or("enabled_roles 只能包含角色名")?;
        if !known.contains(&role) {
            return Err(format!("enabled_roles 含未知角色 {role}"));
        }
        if !seen.insert(role) {
            return Err(format!("enabled_roles 重复了角色 {role}"));
        }
    }
    Ok(known
        .into_iter()
        .filter(|role| seen.contains(role))
        .collect())
}
