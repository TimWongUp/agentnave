//! Read-only recovery of work whose conversation no longer has a writer (ADR-0060).
use super::*;

const ERROR: &str = "owner_exited; previous invocation cannot resume";

fn unfinished(view: &Value) -> bool {
    let director = &view["director"];
    director["round"]["status"] == "running"
        || director["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(director["round"]["seats"].as_array().into_iter().flatten())
            .any(|call| call["status"] == "running" || call["status"] == "pending")
}

fn plain(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    !meta.is_symlink()
}

/// Unlike management, observing ownership must never create a lock or write its contents.
fn probe(root: &Path, id: &str) -> Option<fs::File> {
    let path = crate::db::lock_path(root, id);
    let directory = fs::symlink_metadata(path.parent()?).ok()?;
    let meta = fs::symlink_metadata(&path).ok()?;
    if !directory.is_dir() || !plain(&directory) || !meta.is_file() || !plain(&meta) {
        return None;
    }
    let mut options = fs::File::options();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Do not follow a reparse point or allow replacement while the probe is open.
        options.custom_flags(0x00200000).share_mode(3);
    }
    let file = options.open(&path).ok()?;
    let opened = file.metadata().ok()?;
    if !opened.is_file() || !plain(&opened) {
        return None;
    }
    file.try_lock().ok()?;
    let current = fs::symlink_metadata(&path).ok()?;
    if !current.is_file() || !plain(&current) {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (opened.dev(), opened.ino()) != (current.dev(), current.ino()) {
            return None;
        }
    }
    Some(file)
}

pub(super) fn view(
    reader: &Reader,
    connection: &rusqlite::Connection,
    id: &str,
    rev: i64,
    saved: &Arc<Value>,
) -> Arc<Value> {
    if !unfinished(saved) {
        return saved.clone();
    }
    let Some(lock) = probe(&reader.root, id) else {
        return saved.clone();
    };
    // Already projected at this revision: give the lock straight back, a writer may be asking.
    let projected = reader
        .orphaned
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(id)
        .filter(|(cached, _)| *cached == rev)
        .map(|(_, value)| value.clone());
    if let Some(value) = projected {
        drop(lock);
        return if reader.live().contains_key(id) {
            saved.clone()
        } else {
            value
        };
    }
    // A busy database is insufficient evidence; never wait with the conversation locked.
    if connection.busy_timeout(Duration::ZERO).is_err() {
        return saved.clone();
    }
    // One row under the writer's lock; parsing and projection must not delay its next acquire.
    let latest = connection.query_row(
        "SELECT rev, body FROM views WHERE conversation_id = ?1",
        [id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    );
    drop(lock);
    // The scan goes on with this connection, and its other reads may wait.
    if connection.busy_timeout(crate::db::BUSY).is_err() {
        return saved.clone();
    }
    let Ok((rev, body)) = latest else {
        return saved.clone();
    };
    // A heartbeat may have appeared since scan() read live files, even if its writer let go.
    let fresh = reader.live().contains_key(id);
    let mut cache = reader.orphaned.lock().unwrap_or_else(|p| p.into_inner());
    if !fresh
        && let Some((cached, value)) = cache.get(id)
        && *cached == rev
    {
        return value.clone();
    }
    let Some(mut latest) = serde_json::from_str::<Value>(&body)
        .ok()
        .filter(|value| summary(value).is_some_and(|item| item.id == id))
    else {
        return saved.clone();
    };
    if fresh {
        return Arc::new(latest);
    }
    if unfinished(&latest) {
        recover(&mut latest);
    }
    let latest = Arc::new(latest);
    cache.insert(id.to_owned(), (rev, latest.clone()));
    latest
}

fn interrupt(call: &mut Value) {
    if call["status"] == "running" {
        call["status"] = json!("interrupted");
        if call["error"].as_str().is_none_or(str::is_empty) {
            call["error"] = json!(ERROR);
        }
    }
}

fn recover(view: &mut Value) {
    let director = &mut view["director"];
    if let Some(tasks) = director["tasks"].as_array_mut() {
        for task in tasks {
            interrupt(task);
        }
    }
    let review = director["review"] == true;
    let committed: HashSet<String> = director["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["round"] == director["round"]["number"] && entry["kind"] == "answer")
        .filter_map(|entry| entry["speaker"].as_str().map(str::to_owned))
        .collect();
    let mut kept = Vec::new();
    let number = director["round"]["number"].clone();
    if let Some(round) = director["round"].as_object_mut() {
        let stopped = matches!(
            round.get("status").and_then(Value::as_str),
            Some("cancelled" | "failed")
        );
        if let Some(seats) = round.get_mut("seats").and_then(Value::as_array_mut) {
            for seat in seats {
                interrupt(seat);
                if seat["status"] == "pending" {
                    seat["status"] = json!("not_started");
                } else if review
                    && seat["status"] == "succeeded"
                    && !seat["seat_id"]
                        .as_str()
                        .is_some_and(|id| committed.contains(id))
                    && (seat["review"].is_null() || (stopped && seat["review"] == "pending"))
                {
                    seat["review"] = json!(if stopped { "private" } else { "pending" });
                    if stopped && !seat["output"].is_null() {
                        kept.push(json!({
                            "round": number,
                            "seat_id": seat["seat_id"],
                            "text": seat["output"],
                            "reported_model": seat["reported_model"],
                            "time": seat["finished"],
                            "message_id": null,
                        }));
                    }
                }
            }
        }
        if round
            .get("status")
            .is_some_and(|status| status == "running")
        {
            round.insert("status".into(), json!("interrupted"));
        }
    }
    if let Some(messages) = director["private_messages"].as_array_mut() {
        messages.append(&mut kept);
    }
    let pending = director["round"]["seats"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|seat| seat["review"] == "pending");
    let status = if pending {
        "review"
    } else if let Some(status) = director["round"]["status"].as_str() {
        status
    } else {
        director["tasks"]
            .as_array()
            .and_then(|tasks| tasks.last())
            .and_then(|task| task["status"].as_str())
            .unwrap_or("idle")
    }
    .to_owned();
    if director["round"].is_object() && pending {
        director["round"]["status"] = json!("review");
    }
    if director.get("speaking").is_some() {
        director["speaking"] = json!([]);
    }
    director["status"] = json!(status);
    view["summary"]["status"] = json!(status);
}
