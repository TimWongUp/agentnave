//! Read-only mapping from derived sessions' native boss identity (`boss_key`) to their own
//! subagent workspaces and derived-office projections (ADR-0128 §5, ADR-0130).

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Value, json};

use super::{Group, array, color, iso, label};
use crate::data::boss_app::boss_key;
use crate::data_files::valid_id;
use crate::response::Response;
use crate::terminal::thread_desk_key;

/// Workspace ownership and derived-session boss keys read from SQLite without schema changes.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct DerivedHierarchy {
    pub(crate) workspace_owners: BTreeMap<String, String>,
    pub(crate) derived_boss_keys: BTreeMap<(String, String), String>,
}

impl DerivedHierarchy {
    /// Returns `(parent_workspace_id, parent_desk_key)` when `group_id` is a first-level derived
    /// subagent workspace; respects the one-level nesting invariant (ADR-0109, ADR-0128, ADR-0130).
    pub(crate) fn parent_of(&self, group_id: &str) -> Option<(&str, &str)> {
        let owner = self.workspace_owners.get(group_id)?;
        let ((parent_ws, parent_desk), _) = self
            .derived_boss_keys
            .iter()
            .find(|((p_ws, _), bk)| *bk == owner && p_ws != group_id)?;
        let parent_is_nested = self.workspace_owners.get(parent_ws).is_some_and(|p_owner| {
            self.derived_boss_keys
                .iter()
                .any(|((pp, _), bk)| bk == p_owner && pp != parent_ws)
        });
        (!parent_is_nested).then_some((parent_ws.as_str(), parent_desk.as_str()))
    }
}

pub(crate) fn read_hierarchy(connection: Option<&rusqlite::Connection>) -> DerivedHierarchy {
    let Some(connection) = connection else {
        return DerivedHierarchy::default();
    };
    let mut workspace_owners = BTreeMap::new();
    if let Ok(mut stmt) = connection.prepare("SELECT id, owner FROM workspaces")
        && let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
    {
        for (id, owner) in rows.flatten() {
            if !owner.is_empty() {
                workspace_owners.insert(id, owner);
            }
        }
    }
    if let Ok(mut stmt) = connection.prepare(
        "SELECT DISTINCT manager_session FROM conversations WHERE manager_session IS NOT NULL",
    ) && let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0))
    {
        for ws in rows.flatten() {
            workspace_owners.entry(ws.clone()).or_insert(ws);
        }
    }

    // Only a session that is some workspace's owner can have subagent workspaces, so only those
    // terminal sessions are kept; the rest of the history is filtered out in SQL, body unread.
    let owners: HashSet<&str> = workspace_owners.values().map(String::as_str).collect();
    let mut derived_boss_keys = BTreeMap::new();
    if let Ok(mut stmt) = connection.prepare(
        "SELECT DISTINCT c.manager_session, t.provider, t.session_id \
         FROM tasks t JOIN conversations c ON c.id = t.conversation_id \
         WHERE c.manager_session IS NOT NULL AND t.provider IS NOT NULL \
           AND t.session_id IS NOT NULL AND t.session_id <> '' \
           AND json_extract(t.body, '$.execution_mode') = 'terminal'",
    ) && let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) {
        for (manager_ws, provider, sid) in rows.flatten() {
            let bk = boss_key(&provider, &sid);
            if owners.contains(bk.as_str()) {
                derived_boss_keys
                    .entry((manager_ws, thread_desk_key(&provider, &sid)))
                    .or_insert(bk);
            }
        }
    }
    DerivedHierarchy {
        workspace_owners,
        derived_boss_keys,
    }
}

pub(super) fn enrich_groups_value(groups: &mut [Value], hierarchy: &DerivedHierarchy) {
    let mut by_boss_key: HashMap<String, (String, String)> = HashMap::new();
    for g in groups.iter_mut() {
        let Some(gid) = g["group"].as_str().map(str::to_owned) else {
            continue;
        };
        if let Some(derived) = g["derived"].as_array_mut() {
            for d in derived {
                if let Some(d_key) = d["key"].as_str().map(str::to_owned)
                    && let Some(bk) = hierarchy
                        .derived_boss_keys
                        .get(&(gid.clone(), d_key.clone()))
                {
                    d["boss_key"] = json!(bk);
                    by_boss_key
                        .entry(bk.clone())
                        .or_insert_with(|| (gid.clone(), d_key));
                }
            }
        }
    }
    let is_parent_nested = |parent_ws: &str| {
        hierarchy
            .workspace_owners
            .get(parent_ws)
            .is_some_and(|p_owner| by_boss_key.contains_key(p_owner))
    };
    let mut primary_child: HashMap<(String, String), String> = HashMap::new();
    for g in groups.iter_mut() {
        let Some(gid) = g["group"].as_str().map(str::to_owned) else {
            continue;
        };
        let Some(owner) = hierarchy.workspace_owners.get(&gid) else {
            continue;
        };
        g["owner"] = json!(owner);
        if let Some((parent_ws, parent_desk)) = by_boss_key.get(owner)
            && parent_ws != &gid
            && !is_parent_nested(parent_ws)
        {
            g["parent_workspace_id"] = json!(parent_ws);
            g["parent_desk_key"] = json!(parent_desk);
            if g["handover"] != "pending" {
                primary_child
                    .entry((parent_ws.clone(), parent_desk.clone()))
                    .or_insert(gid);
            }
        }
    }
    for g in groups.iter_mut() {
        let Some(gid) = g["group"].as_str().map(str::to_owned) else {
            continue;
        };
        if let Some(derived) = g["derived"].as_array_mut() {
            for d in derived {
                if let Some(d_key) = d["key"].as_str()
                    && let Some(child_id) = primary_child.get(&(gid.clone(), d_key.to_owned()))
                {
                    d["child_workspace_id"] = json!(child_id);
                }
            }
        }
    }
}

pub(super) fn enrich_requested_group(
    value: &mut Value,
    group_id: &str,
    hierarchy: &DerivedHierarchy,
) {
    let Some(owner) = hierarchy.workspace_owners.get(group_id) else {
        return;
    };
    value["owner"] = json!(owner);
    if let Some((parent_ws, parent_desk)) = hierarchy.parent_of(group_id) {
        value["parent_workspace_id"] = json!(parent_ws);
        value["parent_desk_key"] = json!(parent_desk);
    }
}

/// Encodes a token identically to `chat-terminal.js`'s `encodeToken` for stable `drv-` IDs.
fn encode_token(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() {
            out.push(char::from(byte));
        } else if matches!(
            byte,
            b'_' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' | b'.' | b'-'
        ) {
            out.push_str(&format!("_{byte:02x}"));
        } else {
            out.push_str(&format!("_{byte:02X}"));
        }
    }
    out
}

pub(super) fn derived_row_id(parent_ws: &str, desk_key: &str) -> String {
    format!("drv-{}-{}", encode_token(parent_ws), encode_token(desk_key))
}

// Derived desks never act as meeting seats, so `desk` / `thread` / `to` cover normal traffic;
// `recipients` seat→desk lookup is kept alongside them to stay consistent with frontend `inDesk`.
fn message_belongs_to_desk(
    message: &Value,
    desk_key: &str,
    seat_desks: &HashMap<(String, String), String>,
) -> bool {
    if message["desk"].as_str() == Some(desk_key)
        || (message["thread"]["kind"] == "desk"
            && message["thread"]["key"].as_str() == Some(desk_key))
        || message["to"].as_array().is_some_and(|arr| {
            arr.iter()
                .any(|item| item["key"].as_str() == Some(desk_key))
        })
    {
        return true;
    }
    let Some(conv) = message["conversation"].as_str() else {
        return false;
    };
    message["recipients"].as_array().is_some_and(|arr| {
        arr.iter().any(|item| {
            item["seat"].as_str().is_some_and(|seat| {
                seat_desks
                    .get(&(conv.to_owned(), seat.to_owned()))
                    .is_some_and(|d| d == desk_key)
            })
        })
    })
}

/// Projects the parent workspace context and upstream messages of a derived session desk.
pub(super) fn build_derived_origin(parent: &Group, desk_key: &str) -> Option<Value> {
    let desk = parent.desks.get(desk_key)?;
    let parent_header = parent.header();
    let parent_host = parent_header["host"].as_str();
    let parent_title = parent_header["display_title"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| parent_header["title"].as_str())
        .unwrap_or_default();
    let parent_label = parent_header["label"].as_str().unwrap_or_default();
    let mut seat_desks = HashMap::new();
    for (_, _, room) in &parent.rooms {
        let Some(room_id) = room["id"].as_str() else {
            continue;
        };
        for seat in array(room, "seats") {
            if let (Some(seat_id), Some(d_key)) = (seat["seat"].as_str(), seat["desk"].as_str()) {
                seat_desks.insert((room_id.to_owned(), seat_id.to_owned()), d_key.to_owned());
            }
        }
    }
    let mut messages: Vec<Value> = parent
        .timeline(None, true)
        .into_iter()
        .filter(|msg| message_belongs_to_desk(msg, desk_key, &seat_desks))
        .collect();
    let skip = messages.len().saturating_sub(300);
    if skip > 0 {
        messages.drain(..skip);
    }
    Some(json!({
        "parent_workspace_id": parent.id,
        "parent_desk_key": desk_key,
        "parent_title": parent_title,
        "parent_label": parent_label,
        "desk": desk.value(parent_host),
        "messages": messages,
    }))
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// Parses `/app/derived?workspace=<parent>&desk=<desk>`, returning `None` for other routes.
pub(crate) fn requested_derived(path: &str) -> Option<Result<(String, String), &'static str>> {
    let query = if path == "/app/derived" {
        ""
    } else {
        path.strip_prefix("/app/derived?")?
    };
    let Some(ws_raw) = query_param(query, "workspace") else {
        return Some(Err("需要有效的工作区 ID 和工位"));
    };
    let Some(desk_raw) = query_param(query, "desk") else {
        return Some(Err("需要有效的工作区 ID 和工位"));
    };
    let Some(ws) = crate::usage::percent_decode(ws_raw) else {
        return Some(Err("需要有效的工作区 ID 和工位"));
    };
    let Some(desk) = crate::usage::percent_decode(desk_raw) else {
        return Some(Err("需要有效的工作区 ID 和工位"));
    };
    if !valid_id(&ws)
        || desk.is_empty()
        || desk.len() > 256
        || desk.bytes().any(|b| b < 0x20 || b == 0x7f)
    {
        return Some(Err("需要有效的工作区 ID 和工位"));
    }
    Some(Ok((ws, desk)))
}

/// Builds the read-only derived-office payload for a derived session without a child workspace.
pub(super) fn derived_office_response(
    parent: &Group,
    desk_key: &str,
    hierarchy: Option<&DerivedHierarchy>,
) -> Response {
    let Some(desk) = parent.desks.get(desk_key) else {
        return Response::error(404, "not found");
    };
    if !desk.derived.is_some_and(|(_, derived)| derived) {
        return Response::error(404, "not found");
    }
    let Some(origin) = build_derived_origin(parent, desk_key) else {
        return Response::error(404, "not found");
    };
    let parent_header = parent.header();
    let group_id = derived_row_id(&parent.id, desk_key);
    let provider = desk.provider.as_str();
    let cli_label = label(Some(provider));
    let title = desk
        .identity
        .get("handle_short")
        .or_else(|| desk.identity.get("handle"))
        .or_else(|| desk.identity.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or(desk.role.as_deref().filter(|s| !s.is_empty()))
        .or_else(|| (!desk.model.is_empty()).then_some(desk.model.as_str()))
        .unwrap_or(cli_label);
    let owner = hierarchy
        .and_then(|h| {
            h.derived_boss_keys
                .get(&(parent.id.clone(), desk_key.to_owned()))
        })
        .cloned();
    let updated = desk
        .last
        .or(desk.first)
        .map(iso)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| parent.updated.clone());
    // Derived sessions always run inside an embedded CLI terminal, never a desktop app.
    let derived_app = match provider {
        "claude" => Some(crate::data::boss_app::CLAUDE_CLI),
        "codex" => Some(crate::data::boss_app::CODEX_CLI),
        "antigravity" => Some(crate::data::boss_app::ANTIGRAVITY_CLI),
        _ => None,
    };
    let mut body = json!({
        "group": group_id,
        "derived": true,
        "host": provider,
        "app": Value::Null,
        "label": cli_label,
        "cli_session": crate::git_pr::is_cli_boss(Some(provider), derived_app),
        "title": title,
        "display_title": Value::Null,
        "cli_title": Value::Null,
        "cli_title_custom": false,
        "boss_art_model": desk.art_model(),
        "title_id": Value::Null,
        "handover": Value::Null,
        "took_over": Value::Null,
        "previous_host": Value::Null,
        "previous_label": Value::Null,
        "handover_to": [],
        "handover_from": [],
        "project": parent.project_name,
        "project_key": parent_header["project_key"],
        "project_label": parent_header["project_label"],
        "project_path": parent_header["project_path"],
        "color": color(&group_id),
        "live": desk.active,
        "status": desk.status(),
        "updated": updated,
        "isolated_count": 0,
        "boss_key": format!("{provider}|{cli_label}"),
        "owner": owner,
    });
    if let Value::Object(map) = &mut body {
        map.insert("parent_workspace_id".into(), json!(parent.id));
        map.insert("parent_desk_key".into(), json!(desk_key));
        map.insert("derived_origin".into(), origin);
        map.insert("conversations".into(), json!([]));
        map.insert("members".into(), json!([]));
        map.insert("isolated".into(), json!([]));
        map.insert("desks".into(), json!([]));
        map.insert("optimized".into(), json!({ "count": 0, "desks": [] }));
        map.insert("rooms".into(), json!([]));
        map.insert("messages".into(), json!([]));
        map.insert("boss_messages".into(), json!([]));
        map.insert("omitted".into(), json!(0));
        map.insert("threads".into(), json!([]));
    }
    Response::json(&body)
}
