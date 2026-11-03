//! Reuse the App's saved/live views; the database supplies earlier rounds' full seats,
//! which the view deliberately reduces to identities once a newer round starts.

use super::Reader;
use crate::alerts::{Observation, Readings};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Hold the view as well as its projection, so unchanged inputs need no round JSON parsing.
pub(super) type Cache = HashMap<String, (Arc<Value>, Vec<Observation>)>;

impl Reader {
    pub(crate) fn alert_observations(&self) -> Result<Readings, String> {
        // A failed database read must not establish an empty startup baseline.
        let database = crate::db::read(&self.root)?;
        let (_, snapshots) = self.scan();
        let mut cache = self.alerts.lock().unwrap_or_else(|p| p.into_inner());
        let mut events = Readings::new();
        let mut seen = HashSet::new();
        for snapshot in &snapshots {
            let id = &snapshot.item.id;
            seen.insert(id.clone());
            if !cache
                .get(id)
                .is_some_and(|(view, _)| Arc::ptr_eq(view, &snapshot.view))
            {
                let rounds = if snapshot.item.mode == "show" {
                    match saved_rounds(database.as_ref(), id) {
                        Ok(rounds) => rounds,
                        Err(_) => {
                            cache.remove(id);
                            continue;
                        }
                    }
                } else {
                    Vec::new()
                };
                let observations = crate::alerts::observations(snapshot, &rounds);
                cache.insert(id.clone(), (snapshot.view.clone(), observations));
            }
            if let Some((_, observations)) = cache.get(id) {
                events.insert(id.clone(), observations.clone());
            }
        }
        cache.retain(|id, _| seen.contains(id));
        Ok(events)
    }
}

fn saved_rounds(connection: Option<&rusqlite::Connection>, id: &str) -> Result<Vec<Value>, String> {
    let Some(connection) = connection else {
        return Ok(Vec::new());
    };
    let mut query = connection
        .prepare("SELECT body FROM rounds WHERE conversation_id = ?1 ORDER BY number")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.map(|row| {
        let body = row.map_err(|e| e.to_string())?;
        serde_json::from_str(&body).map_err(|e| e.to_string())
    })
    .collect()
}
