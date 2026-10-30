//! Tray-only cleanup and renaming in the conversation database (ADR-0012, ADR-0026). Every
//! change happens under the conversation's OS lock, the same one its writing server holds.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::sync::LazyLock;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};

use crate::data::Overview;

const RETRY: Duration = Duration::from_millis(100);
const ATTEMPTS: usize = 11;

/// What cleanup needs to know about one saved conversation.
struct State {
    title: String,
    kind: String,
    /// Any round still holds an answer waiting for review (RoundRecord.pending_review()).
    pending: bool,
    /// The status the last saved view showed; "running" left by a crashed writer included.
    status: String,
    /// When that view was written, in milliseconds since the epoch.
    written_ms: Option<i64>,
}

fn read_state(connection: &Connection, id: &str) -> Result<Option<State>, String> {
    let row = connection
        .query_row(
            "SELECT title, kind, \
             EXISTS (SELECT 1 FROM rounds, json_each(rounds.body, '$.seats') AS seat \
                     WHERE rounds.conversation_id = conversations.id \
                     AND json_extract(seat.value, '$.review') = 'pending'), \
             (SELECT body FROM views WHERE conversation_id = conversations.id), \
             (SELECT written_ms FROM views WHERE conversation_id = conversations.id) \
             FROM conversations WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((title, kind, pending, view, written_ms)) = row else {
        return Ok(None);
    };
    let status = view
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .and_then(|view| view["summary"]["status"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "idle".to_owned());
    Ok(Some(State {
        title,
        kind,
        pending,
        status,
        written_ms,
    }))
}

fn lock_file(root: &Path, id: &str) -> io::Result<File> {
    let path = crate::db::lock_path(root, id);
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() => return Err(io::Error::other("锁不是普通文件")),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn result(id: &str, status: &str, message: &str) -> Value {
    json!({"id": id, "status": status, "message": message})
}

/// A writing connection and the conversation's lock, with its state read under that lock;
/// or the result to report instead.
fn locked(root: &Path, id: &str) -> Result<(Connection, File, State), Value> {
    let connection = match crate::db::write(root) {
        Ok(Some(connection)) => connection,
        Ok(None) => return Err(result(id, "missing", "会话已不存在")),
        Err(e) => return Err(result(id, "error", &e)),
    };
    // Unknown ids never create a lock file.
    match read_state(&connection, id) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(result(id, "missing", "会话已不存在")),
        Err(e) => return Err(result(id, "error", &e)),
    }
    let lock = lock_file(root, id).map_err(|e| result(id, "error", &e.to_string()))?;
    // Probe locks are short-lived. Retry for at most one second before reporting contention.
    for attempt in 0..ATTEMPTS {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if attempt + 1 < ATTEMPTS => thread::sleep(RETRY),
            Err(TryLockError::WouldBlock) => {
                return Err(result(id, "in_use", "正在被某个宿主使用，请空闲后重试"));
            }
            Err(e) => return Err(result(id, "error", &e.to_string())),
        }
    }
    // Re-read only after acquiring the lock. A saved running status now means a crashed writer.
    match read_state(&connection, id) {
        Ok(Some(state)) => Ok((connection, lock, state)),
        Ok(None) => Err(result(id, "missing", "会话已不存在")),
        Err(e) => Err(result(id, "error", &e)),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The widest title in columns; the server holds a boss's titles to the same width.
const TITLE_WIDTH: usize = 48;
// Unicode 15.0 W/F intervals, shared with the MCP title validator.
static TITLE_WIDE_RANGES: LazyLock<Vec<[u32; 2]>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../src/agentnave/title-width.json"))
        .expect("valid bundled title widths")
});

/// Columns a title takes: East Asian wide and fullwidth characters count two, others one.
fn title_width(title: &str) -> usize {
    title
        .chars()
        .map(|c| {
            if TITLE_WIDE_RANGES
                .iter()
                .any(|[low, high]| *low <= c as u32 && c as u32 <= *high)
            {
                2
            } else {
                1
            }
        })
        .sum()
}

/// Rename an idle conversation; servers reload it once its revision changes.
pub fn rename(root: &Path, id: &str, title: &str) -> Value {
    let title = title.trim();
    if title.is_empty() || title_width(title) > TITLE_WIDTH || title.chars().any(char::is_control) {
        return result(
            id,
            "error",
            "标题不能为空、不含换行，最多 24 个汉字或 48 个英文字符",
        );
    }
    let (mut connection, _lock, _state) = match locked(root, id) {
        Ok(found) => found,
        Err(report) => return report,
    };
    let renamed = (|| -> rusqlite::Result<()> {
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE conversations SET title = ?1, rev = rev + 1 WHERE id = ?2",
            rusqlite::params![title, id],
        )?;
        // The derived view follows at once so the office shows the new title before any server
        // projects it again; an unreadable view is left for the next server to replace.
        let view: Option<String> = transaction
            .query_row(
                "SELECT body FROM views WHERE conversation_id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(mut view) = view.and_then(|body| serde_json::from_str::<Value>(&body).ok()) {
            for key in ["summary", "director", "public"] {
                if view[key].is_object() {
                    view[key]["title"] = json!(title);
                }
            }
            transaction.execute(
                "UPDATE views SET body = ?1, rev = rev + 1, written_ms = ?2 \
                 WHERE conversation_id = ?3",
                rusqlite::params![view.to_string(), now_ms(), id],
            )?;
        }
        transaction.commit()
    })();
    match renamed {
        Ok(()) => json!({"id": id, "status": "renamed", "title": title, "message": "已改名"}),
        Err(e) => result(id, "error", &format!("保存标题失败：{e}")),
    }
}

pub fn delete(root: &Path, id: &str) -> Value {
    let (mut connection, _lock, state) = match locked(root, id) {
        Ok(found) => found,
        Err(report) => return report,
    };
    if state.pending {
        return result(id, "pending_review", "有待审回答，请先由 boss 处理");
    }
    // One transaction, so the workspace's handover never outlives its last conversation.
    let deleted = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .and_then(|transaction| {
            transaction.execute("DELETE FROM conversations WHERE id = ?1", [id])?;
            forget_emptied(&transaction)?;
            transaction.commit()
        });
    match deleted {
        Ok(()) => result(id, "deleted", "已删除会话记录；席位文件保留"),
        Err(e) => result(id, "error", &format!("删除失败：{e}")),
    }
}

/// Drop the handover of a workspace whose last conversation is gone, so its bosses are free
/// again (ADR-0033). A database an older server created may lack the tables.
fn forget_emptied(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(include_str!("../../../src/agentnave/schema.sql"))?;
    connection.execute_batch(
        "DELETE FROM workspaces WHERE NOT EXISTS \
         (SELECT 1 FROM conversations WHERE manager_session = workspaces.id); \
         DELETE FROM former_bosses WHERE workspace NOT IN (SELECT id FROM workspaces);",
    )
}

/// A workspace keyed by a boss chat; legacy identities have none.
fn boss_workspace(id: &str) -> bool {
    id.strip_prefix("s-")
        .is_some_and(|key| key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Mark a workspace as waiting for another boss (ADR-0033). Its boss keeps it until a new chat
/// takes it over, so this needs no conversation lock.
pub fn handover(root: &Path, group: &str) -> Value {
    if !boss_workspace(group) {
        return result(group, "error", "这个工作区没有记录 boss 聊天，不能交接");
    }
    let connection = match crate::db::write(root) {
        Ok(Some(connection)) => connection,
        Ok(None) => return result(group, "missing", "工作区已不存在"),
        Err(e) => return result(group, "error", &e),
    };
    let marked = (|| -> rusqlite::Result<bool> {
        let found: bool = connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM conversations WHERE manager_session = ?1)",
            [group],
            |row| row.get(0),
        )?;
        if !found {
            return Ok(false);
        }
        // A database an older server created may still lack the table.
        connection.execute_batch(include_str!("../../../src/agentnave/schema.sql"))?;
        connection.execute(
            "INSERT INTO workspaces (id, owner, owner_host, pending, released) \
             VALUES (?1, ?1, (SELECT host FROM conversations WHERE manager_session = ?1 \
             ORDER BY created DESC, id DESC LIMIT 1), 1, strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')) \
             ON CONFLICT (id) DO UPDATE SET pending = 1, note = NULL, released = excluded.released, \
             owner_host = coalesce(workspaces.owner_host, excluded.owner_host)",
            [group],
        )?;
        Ok(true)
    })();
    match marked {
        Ok(true) => result(
            group,
            "waiting",
            "已标为待接手。在任意宿主开一个新聊天，请它接手这个工作区",
        ),
        Ok(false) => result(group, "missing", "工作区已不存在"),
        Err(e) => result(group, "error", &format!("交接失败：{e}")),
    }
}

/// Current members of the anchor's workspace; legacy identities remain individual records.
fn workspace_ids(connection: &Connection, anchor: &str) -> Result<Vec<String>, String> {
    let session: Option<Option<String>> = connection
        .query_row(
            "SELECT manager_session FROM conversations WHERE id = ?1",
            [anchor],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(session) = session else {
        return Ok(Vec::new());
    };
    let Some(session) = session.filter(|s| boss_workspace(s)) else {
        return Ok(vec![anchor.to_owned()]);
    };
    connection
        .prepare("SELECT id FROM conversations WHERE manager_session = ?1 ORDER BY id")
        .and_then(|mut query| query.query_map([session], |row| row.get(0))?.collect())
        .map_err(|e| e.to_string())
}

/// Remove one quiet workspace only if the confirmed members and their activity are unchanged.
pub fn cleanup(root: &Path, ids: &[String], cutoff: i64) -> Vec<Value> {
    let report =
        |status: &str, message: &str| ids.iter().map(|id| result(id, status, message)).collect();
    let mut expected = ids.to_vec();
    expected.sort();
    expected.dedup();
    let Some(anchor) = expected.first() else {
        return Vec::new();
    };
    let mut connection = match crate::db::write(root) {
        Ok(Some(connection)) => connection,
        Ok(None) => return report("missing", "会话已不存在"),
        Err(e) => return report("error", &e),
    };
    match workspace_ids(&connection, anchor) {
        Ok(current) if current == expected => {}
        Ok(_) => return report("changed", "工作区的会话已变化，已保留；请刷新后重试"),
        Err(e) => return report("error", &e),
    }
    // Acquire OS locks before the DB write transaction so a writer can finish without deadlock.
    let mut locks = Vec::new();
    for id in &expected {
        match locked(root, id) {
            Ok((_, lock, state)) if !state.pending => locks.push(lock),
            Ok(_) => return report("pending_review", "工作区有待审回答，已整组保留"),
            Err(error) => {
                return report(
                    error["status"].as_str().unwrap_or("error"),
                    error["message"].as_str().unwrap_or("工作区暂时不能清理"),
                );
            }
        }
    }
    let removed = (|| -> Result<bool, String> {
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if workspace_ids(&transaction, anchor)? != expected {
            return Ok(false);
        }
        for id in &expected {
            let (created, body): (String, Option<String>) = transaction.query_row(
                "SELECT created, views.body FROM conversations LEFT JOIN views ON views.conversation_id = conversations.id WHERE conversations.id = ?1",
                [id], |row| Ok((row.get(0)?, row.get(1)?))
            ).map_err(|e| e.to_string())?;
            let created = crate::stage::timestamp(&created);
            let updated = body
                .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                .and_then(|view| {
                    view["summary"]["updated"]
                        .as_str()
                        .and_then(crate::stage::timestamp)
                });
            let Some((created, updated)) = created.zip(updated) else {
                return Ok(false);
            };
            if created.max(updated) / 1_000 >= cutoff {
                return Ok(false);
            }
        }
        for id in &expected {
            transaction
                .execute("DELETE FROM conversations WHERE id = ?1", [id])
                .map_err(|e| e.to_string())?;
        }
        forget_emptied(&transaction).map_err(|e| e.to_string())?;
        transaction.commit().map_err(|e| e.to_string())?;
        Ok(true)
    })();
    match removed {
        Ok(true) => report("deleted", "已清理工作区历史；席位文件保留"),
        Ok(false) => report("changed", "工作区已有新活动或时间无法确认，已整组保留"),
        Err(e) => report("error", &format!("清理失败：{e}")),
    }
}

pub fn list(root: &Path, overview: Overview) -> Value {
    let connection = match crate::db::read(root) {
        Ok(Some(connection)) => connection,
        Ok(None) => return json!({"items": [], "unavailable": [], "missing": true}),
        Err(e) => return json!({"error": e}),
    };
    let ids: Result<Vec<String>, _> = connection
        .prepare("SELECT id FROM conversations")
        .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect());
    let ids = match ids {
        Ok(ids) => ids,
        Err(e) => return json!({"error": e.to_string()}),
    };
    let mut items = Vec::new();
    let mut unavailable = Vec::new();
    for id in ids {
        if !crate::data::valid_id(&id) {
            continue;
        }
        let lock = lock_file(root, &id);
        let probe = lock.as_ref().map(|file| file.try_lock());
        let (held, error) = match probe {
            Ok(Ok(())) => (true, None),
            Ok(Err(TryLockError::WouldBlock)) => (false, None),
            Ok(Err(e)) => (false, Some(e.to_string())),
            Err(e) => (false, Some(e.to_string())),
        };
        let state = match read_state(&connection, &id) {
            Ok(Some(state)) => state,
            Ok(None) => continue, // deleted since the listing
            Err(e) => {
                unavailable.push(json!({"id": id, "reason": e}));
                continue;
            }
        };
        let snapshot = overview.items.iter().find(|item| item.id == id);
        let status = if held && state.status == "running" {
            "interrupted"
        } else if state.pending {
            "review"
        } else {
            &state.status
        };
        let reason = if let Some(error) = error {
            error
        } else if state.pending {
            "有待审回答，请先由 boss 处理".to_owned()
        } else if !held {
            "正在被某个宿主使用，请空闲后重试".to_owned()
        } else {
            String::new()
        };
        let updated = snapshot
            .map(|item| item.updated.as_str())
            .unwrap_or_default();
        items.push(json!({
            "id": id, "title": state.title, "mode": state.kind,
            "status": status, "updated": updated, "updated_ms": state.written_ms,
            "deletable": reason.is_empty(), "reason": reason,
            "group": snapshot.and_then(crate::stage::chat_group),
        }));
        // The probe's File drops here, before reading the next conversation.
    }
    json!({"items": items, "unavailable": unavailable, "missing": false})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Reader;

    #[test]
    fn cleanup_rechecks_recent_siblings_before_deleting_old_history() {
        let root = tempfile::tempdir().unwrap();
        for id in ["old", "newly-finished"] {
            fixture(root.path(), id);
            set(
                root.path(),
                "UPDATE conversations SET created = '2020-01-01T00:00:00Z', manager_session = 's-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' WHERE id = ?1",
                id,
            );
            set(
                root.path(),
                "UPDATE views SET body = json_set(body, '$.summary.updated', '2020-01-01T00:00:00Z') WHERE conversation_id = ?1",
                id,
            );
        }
        let ids = vec!["old".to_owned(), "newly-finished".to_owned()];
        let cutoff = crate::stage::timestamp("2026-10-01T00:00:00Z").unwrap() / 1_000;
        // A conversation created since the preview preserves the whole workspace.
        assert!(
            cleanup(root.path(), &ids[..1], cutoff)
                .iter()
                .all(|r| r["status"] == "changed")
        );
        set(
            root.path(),
            "UPDATE views SET body = json_set(body, '$.summary.updated', '2026-10-25T00:00:00Z') WHERE conversation_id = ?1",
            "newly-finished",
        );
        assert!(
            cleanup(root.path(), &ids, cutoff)
                .iter()
                .all(|r| r["status"] == "changed")
        );
        assert_eq!(count(root.path(), "conversations", "old"), 1);
        assert_eq!(count(root.path(), "conversations", "newly-finished"), 1);
        set(
            root.path(),
            "DELETE FROM views WHERE conversation_id = ?1",
            "newly-finished",
        );
        assert!(
            cleanup(root.path(), &ids, cutoff)
                .iter()
                .all(|r| r["status"] == "changed")
        );
        set(
            root.path(),
            "UPDATE conversations SET manager_session = NULL WHERE id = ?1",
            "newly-finished",
        );
        let workspace = "s-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(handover(root.path(), workspace)["status"], "waiting");
        assert!(
            cleanup(root.path(), &ids[..1], cutoff)
                .iter()
                .all(|r| r["status"] == "deleted")
        );
        // The emptied workspace's handover goes with its last conversation.
        let handovers: i64 = crate::db::read(root.path())
            .unwrap()
            .unwrap()
            .query_row("SELECT count(*) FROM workspaces", [], |row| row.get(0))
            .unwrap();
        assert_eq!(handovers, 0);
        assert_eq!(delete(root.path(), "newly-finished")["status"], "deleted");
    }

    /// A saved conversation with a running task, its view, and an isolated seat's files.
    fn fixture(root: &Path, id: &str) {
        crate::db::put(
            root,
            id,
            &json!({"kind": "task", "title": "测试", "tasks": [{"status": "running"}]}),
            &json!({"version": 1, "summary": {"id": id, "title": "测试", "status": "running"},
                "director": {"title": "测试"}, "public": null}),
        );
        let seat = root.join("seats").join(id).join("a");
        fs::create_dir_all(&seat).unwrap();
        fs::write(seat.join("output.txt"), "keep").unwrap();
    }

    fn set(root: &Path, sql: &str, id: &str) {
        crate::db::write(root)
            .unwrap()
            .unwrap()
            .execute(sql, [id])
            .unwrap();
    }

    fn count(root: &Path, table: &str, id: &str) -> i64 {
        let column = if table == "conversations" {
            "id"
        } else {
            "conversation_id"
        };
        crate::db::read(root)
            .unwrap()
            .unwrap()
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                [id],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// A process another test spawns may briefly inherit the tray's descriptor until its
    /// exec closes it, so allow the same short retry as cleanup itself.
    fn take(lock: &File) {
        let mut held = lock.try_lock();
        for _ in 1..ATTEMPTS {
            if !matches!(held, Err(TryLockError::WouldBlock)) {
                break;
            }
            thread::sleep(RETRY);
            held = lock.try_lock();
        }
        held.unwrap();
    }

    #[test]
    fn cleanup_cascades_rows_and_keeps_lock_inode_and_seat_files() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "idle");
        set(
            root.path(),
            "DELETE FROM tasks WHERE conversation_id = ?1",
            "idle",
        );
        let lock = lock_file(root.path(), "idle").unwrap();
        assert_eq!(delete(root.path(), "idle")["status"], "deleted");
        for table in ["conversations", "tasks", "views"] {
            assert_eq!(count(root.path(), table, "idle"), 0);
        }
        assert!(root.path().join("seats/idle/a/output.txt").is_file());
        // A handle opened before cleanup still excludes a handle opened afterward.
        take(&lock);
        assert!(matches!(
            lock_file(root.path(), "idle").unwrap().try_lock(),
            Err(TryLockError::WouldBlock)
        ));
        drop(lock);
        assert_eq!(delete(root.path(), "idle")["status"], "missing");
        assert_eq!(delete(root.path(), "never")["status"], "missing");
        assert!(!crate::db::lock_path(root.path(), "never").exists());
    }

    #[test]
    fn rename_rewrites_conversation_and_view_under_the_lock() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "named");
        assert_eq!(
            rename(root.path(), "named", " 新标题 ")["status"],
            "renamed"
        );
        let connection = crate::db::read(root.path()).unwrap().unwrap();
        let (title, rev): (String, i64) = connection
            .query_row(
                "SELECT title, rev FROM conversations WHERE id = 'named'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((title.as_str(), rev), ("新标题", 2)); // servers reload the new revision
        assert_eq!(count(root.path(), "tasks", "named"), 1); // everything else kept
        let view: String = connection
            .query_row(
                "SELECT body FROM views WHERE conversation_id = 'named'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let view: Value = serde_json::from_str(&view).unwrap();
        assert_eq!(view["summary"]["title"], "新标题");
        assert_eq!(view["director"]["title"], "新标题");
        assert!(view["public"].is_null());
        assert_eq!(rename(root.path(), "named", "a\nb")["status"], "error");
        assert_eq!(rename(root.path(), "named", " ")["status"], "error");
        assert_eq!(
            rename(root.path(), "named", &"汉".repeat(25))["status"],
            "error"
        );
        assert_eq!(
            rename(root.path(), "named", &"🚀".repeat(25))["status"],
            "error"
        );
        let lock = lock_file(root.path(), "named").unwrap();
        take(&lock);
        assert_eq!(rename(root.path(), "named", "被占用")["status"], "in_use");
        drop(lock);
        assert_eq!(rename(root.path(), "gone", "x")["status"], "missing");
    }

    #[test]
    fn locked_and_pending_records_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "busy");
        let lock = lock_file(root.path(), "busy").unwrap();
        lock.try_lock().unwrap();
        assert_eq!(delete(root.path(), "busy")["status"], "in_use");
        drop(lock);
        crate::db::put(
            root.path(),
            "busy",
            &json!({"kind": "show", "rounds": [
                {"status": "finished", "seats": [{"review": "pending"}]},
                {"status": "running", "seats": []}]}),
            &json!({"version": 1, "summary": {"id": "busy", "status": "running"}}),
        );
        assert_eq!(delete(root.path(), "busy")["status"], "pending_review");
        assert_eq!(count(root.path(), "conversations", "busy"), 1);
    }

    #[test]
    fn protocol_requires_post_action_and_valid_batch() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "a1");
        let reader = Reader::new(root.path().to_owned());
        let route = "/app/conversations/delete";
        for method in ["GET", "PUT", "DELETE", "OPTIONS"] {
            assert_eq!(
                reader
                    .request(method, route, Some("1"), br#"{"ids":["a1"]}"#)
                    .status,
                405
            );
        }
        assert_eq!(
            reader
                .request("POST", route, None, br#"{"ids":["a1"]}"#)
                .status,
            403
        );
        for body in [
            br#"{"ids":["a1","../outside"]}"#.as_slice(),
            b"{}",
            b"invalid",
            br#"{"ids":[]}"#,
            br#"{"ids":["a1"],"updated_before":"yesterday"}"#,
        ] {
            assert_eq!(reader.request("POST", route, Some("1"), body).status, 400);
        }
        assert_eq!(count(root.path(), "conversations", "a1"), 1);
        let response = reader.request("POST", route, Some("1"), br#"{"ids":["a1","gone"]}"#);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["results"][0]["status"], "deleted");
        assert_eq!(body["results"][1]["status"], "missing");
        assert_eq!(
            reader
                .request("POST", "/manage/state", Some("1"), b"{}")
                .status,
            405
        );
    }

    #[test]
    fn management_lists_conversations_and_probes_locks() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "stale");
        fixture(root.path(), "busy");
        let lock = lock_file(root.path(), "busy").unwrap();
        lock.try_lock().unwrap();
        let reader = Reader::new(root.path().to_owned());
        let response = reader.respond("/manage/state");
        let payload: Value = serde_json::from_slice(&response.body).unwrap();
        let items = payload["items"].as_array().unwrap();
        let stale = items.iter().find(|v| v["id"] == "stale").unwrap();
        assert_eq!(stale["status"], "interrupted");
        assert_eq!(stale["deletable"], true);
        assert!(stale["updated_ms"].as_i64().is_some());
        let busy = items.iter().find(|v| v["id"] == "busy").unwrap();
        assert_eq!(busy["status"], "running");
        assert_eq!(busy["deletable"], false);
        // A crashed writer's running record is deletable.
        assert_eq!(delete(root.path(), "stale")["status"], "deleted");
        let after = list(root.path(), reader.overview());
        assert_eq!(after["items"].as_array().unwrap().len(), 1);
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(list(empty.path(), Overview::default())["missing"], true);
    }

    #[test]
    fn unreadable_rounds_keep_the_conversation() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "broken");
        crate::db::write(root.path())
            .unwrap()
            .unwrap()
            .execute(
                "INSERT INTO rounds (id, conversation_id, number, body) \
                 VALUES ('r', 'broken', 1, '{broken')",
                [],
            )
            .unwrap();
        assert_eq!(delete(root.path(), "broken")["status"], "error");
        assert_eq!(count(root.path(), "conversations", "broken"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_lock_cannot_redirect_cleanup() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "original");
        let target = root.path().join("other.json");
        fs::write(&target, "keep").unwrap();
        let lock = crate::db::lock_path(root.path(), "original");
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        symlink(&target, &lock).unwrap();
        assert_eq!(delete(root.path(), "original")["status"], "error");
        assert_eq!(count(root.path(), "conversations", "original"), 1);
        assert_eq!(fs::read_to_string(target).unwrap(), "keep");
    }

    #[cfg(unix)]
    #[test]
    fn python_flock_excludes_rust_and_cleanup_retries_then_rereads() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "python");
        // Any Python 3 works: only the standard library's fcntl and sqlite3 are used, as the
        // servers do.
        let mut child = Command::new("python3")
            .arg("-c")
            .arg(
                r#"
import fcntl, json, pathlib, sqlite3, sys
root = pathlib.Path(sys.argv[1])
with (root / 'locks' / 'python.lock').open('a+b') as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    print('locked', flush=True)
    sys.stdin.readline()
    with sqlite3.connect(root / 'agentnave.db') as db:
        db.execute("INSERT INTO rounds (id, conversation_id, number, body) VALUES "
                   "('r', 'python', 1, ?)", (json.dumps({'seats': [{'review': 'pending'}]}),))
"#,
            )
            .arg(root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        fs::create_dir_all(root.path().join("locks")).unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "locked");
        assert_eq!(delete(root.path(), "python")["status"], "in_use");
        let result = thread::scope(|scope| {
            let deleting = scope.spawn(|| delete(root.path(), "python"));
            thread::sleep(Duration::from_millis(150));
            child.stdin.take().unwrap().write_all(b"release\n").unwrap();
            assert!(child.wait().unwrap().success());
            deleting.join().unwrap()
        });
        assert_eq!(result["status"], "pending_review");
        assert_eq!(count(root.path(), "conversations", "python"), 1);
    }
}
