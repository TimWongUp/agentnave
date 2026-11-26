//! Read-only mapping from derived sessions' native boss identity (`boss_key`) to their own
//! subagent workspaces (ADR-0127 §5).

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use crate::data::boss_app::boss_key;
use crate::terminal::thread_desk_key;

/// Workspace ownership and derived-session boss keys read from SQLite without schema changes.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct DerivedHierarchy {
    pub(crate) workspace_owners: BTreeMap<String, String>,
    pub(crate) derived_boss_keys: BTreeMap<(String, String), String>,
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

    let mut derived_boss_keys = BTreeMap::new();
    if let Ok(mut stmt) = connection.prepare(
        "SELECT c.manager_session, t.provider, t.session_id, t.body \
         FROM tasks t JOIN conversations c ON c.id = t.conversation_id \
         WHERE c.manager_session IS NOT NULL AND t.provider IS NOT NULL AND t.session_id IS NOT NULL",
    ) && let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    }) {
        for (manager_ws, provider, sid, body) in rows.flatten() {
            if sid.is_empty() {
                continue;
            }
            let Ok(parsed) = serde_json::from_str::<Value>(&body) else {
                continue;
            };
            if parsed["execution_mode"] != "terminal" {
                continue;
            }
            let desk = thread_desk_key(&provider, &sid);
            derived_boss_keys
                .entry((manager_ws, desk))
                .or_insert_with(|| boss_key(&provider, &sid));
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
    let Some(((parent_ws, parent_desk), _)) = hierarchy
        .derived_boss_keys
        .iter()
        .find(|((p_ws, _), bk)| *bk == owner && p_ws != group_id)
    else {
        return;
    };
    let parent_is_nested = hierarchy
        .workspace_owners
        .get(parent_ws)
        .is_some_and(|p_owner| {
            hierarchy
                .derived_boss_keys
                .iter()
                .any(|((pp, _), bk)| bk == p_owner && pp != parent_ws)
        });
    if !parent_is_nested {
        value["parent_workspace_id"] = json!(parent_ws);
        value["parent_desk_key"] = json!(parent_desk);
    }
}
