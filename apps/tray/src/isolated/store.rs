//! SQLite read/write access to `<data_root>/worktrees.db` (`SCHEMA_VERSION = 1`).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};

use super::Dir;
use super::leases::probe_in_use;

pub(crate) const NAME: &str = "worktrees.db";
/// The service's `SCHEMA_VERSION` this build understands.
pub(crate) const VERSION: i64 = 1;
pub(crate) const MANAGED_BRANCH_PREFIX: &str = "refs/heads/agentnave/";
const BUSY: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub(crate) struct Record {
    pub id: String,
    pub workspace_id: String,
    pub repo_common_dir: String,
    pub main_root: String,
    pub worktree_path: String,
    pub worktree_git_dir: Option<String>,
    pub branch_ref: String,
    pub base_oid: String,
    pub state: String,
    pub usage_generation: i64,
    pub attention: Option<String>,
    pub confirmation_digest: Option<String>,
    pub delete_oid: Option<String>,
}

impl Record {
    pub(crate) fn branch(&self) -> &str {
        self.branch_ref
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.branch_ref)
    }
}

pub(crate) fn now_iso() -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX));
    crate::time_format::iso(micros)
}

pub(crate) fn read_dirs(root: &Path) -> rusqlite::Result<Vec<Dir>> {
    let path = root.join(NAME);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(1))?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != VERSION {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(
        "SELECT workspace_id, worktree_path, branch_ref, state, id, created_at, \
         repo_common_dir, attention FROM isolated_dirs \
         WHERE state IN ('ready', 'deleting', 'needs_attention') ORDER BY created_at, id",
    )?;
    statement
        .query_map([], |row| {
            let id: String = row.get(4)?;
            let branch: String = row.get(2)?;
            let state: String = row.get(3)?;
            let common_dir: String = row.get(6)?;
            let leased = probe_in_use(Path::new(&common_dir), &id);
            Ok(Dir {
                workspace: row.get(0)?,
                path: row.get(1)?,
                branch: branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&branch)
                    .to_owned(),
                ready: state == "ready",
                leased,
                attention: row.get(7)?,
                id,
                created: row.get(5)?,
            })
        })?
        .collect()
}

pub(crate) struct WorktreeStore {
    path: PathBuf,
}

impl WorktreeStore {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            path: root.join(NAME),
        }
    }

    fn open(&self, flags: OpenFlags) -> Result<Option<Connection>, String> {
        if !self.path.is_file() {
            return Ok(None);
        }
        let connection =
            Connection::open_with_flags(&self.path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)
                .map_err(|e| e.to_string())?;
        connection.busy_timeout(BUSY).map_err(|e| e.to_string())?;
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if version != VERSION {
            return Err(format!(
                "worktrees.db 版本为 {version}，当前 App 只支持 {VERSION}；请重连宿主或更新 App"
            ));
        }
        Ok(Some(connection))
    }

    fn write(&self, sql: &str, params: impl rusqlite::Params) -> Result<bool, String> {
        let Some(mut connection) = self.open(OpenFlags::SQLITE_OPEN_READ_WRITE)? else {
            return Ok(false);
        };
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = tx.execute(sql, params).map_err(|e| e.to_string())? > 0;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(changed)
    }

    pub(crate) fn get(&self, isolation_id: &str) -> Result<Option<Record>, String> {
        let Some(connection) = self.open(OpenFlags::SQLITE_OPEN_READ_ONLY)? else {
            return Ok(None);
        };
        connection
            .query_row(
                "SELECT id, workspace_id, repo_common_dir, main_root, worktree_path, \
                 worktree_git_dir, branch_ref, base_oid, state, usage_generation, \
                 attention, confirmation_digest, delete_oid \
                 FROM isolated_dirs WHERE id = ?1",
                [isolation_id],
                row_to_record,
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub(crate) fn all_records(&self) -> Result<Vec<Record>, String> {
        let Some(connection) = self.open(OpenFlags::SQLITE_OPEN_READ_ONLY)? else {
            return Ok(Vec::new());
        };
        let mut stmt = connection
            .prepare(
                "SELECT id, workspace_id, repo_common_dir, main_root, worktree_path, \
                 worktree_git_dir, branch_ref, base_oid, state, usage_generation, \
                 attention, confirmation_digest, delete_oid \
                 FROM isolated_dirs ORDER BY created_at, id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], row_to_record)
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())
    }

    pub(crate) fn save_confirmation(
        &self,
        isolation_id: &str,
        digest: &str,
        updated_at: &str,
    ) -> Result<bool, String> {
        self.write(
            "UPDATE isolated_dirs SET confirmation_digest = ?1, updated_at = ?2 \
             WHERE id = ?3 AND state IN ('ready', 'needs_attention', 'deleting')",
            rusqlite::params![digest, updated_at, isolation_id],
        )
    }

    pub(crate) fn mark_deleting(
        &self,
        isolation_id: &str,
        phase: &str,
        delete_oid: &str,
        updated_at: &str,
        expected_state: &str,
    ) -> Result<bool, String> {
        self.write(
            "UPDATE isolated_dirs SET state = 'deleting', phase = ?1, delete_oid = ?2, \
             attention = NULL, updated_at = ?3 WHERE id = ?4 AND state = ?5",
            rusqlite::params![phase, delete_oid, updated_at, isolation_id, expected_state],
        )
    }

    pub(crate) fn resume_creating(
        &self,
        isolation_id: &str,
        phase: &str,
        updated_at: &str,
    ) -> Result<bool, String> {
        self.write(
            "UPDATE isolated_dirs SET state = 'creating', phase = ?1, attention = NULL, \
             updated_at = ?2 WHERE id = ?3 AND state = 'needs_attention'",
            rusqlite::params![phase, updated_at, isolation_id],
        )
    }

    pub(crate) fn update_phase(
        &self,
        isolation_id: &str,
        phase: &str,
        updated_at: &str,
    ) -> Result<bool, String> {
        self.write(
            "UPDATE isolated_dirs SET phase = ?1, updated_at = ?2 \
             WHERE id = ?3 AND state = 'deleting'",
            rusqlite::params![phase, updated_at, isolation_id],
        )
    }

    pub(crate) fn mark_attention(
        &self,
        isolation_id: &str,
        expected_state: &str,
        phase: &str,
        reason: &str,
        updated_at: &str,
    ) -> Result<Option<Record>, String> {
        self.write(
            "UPDATE isolated_dirs SET state = 'needs_attention', phase = ?1, attention = ?2, \
             confirmation_digest = NULL, updated_at = ?3 WHERE id = ?4 AND state = ?5",
            rusqlite::params![phase, reason, updated_at, isolation_id, expected_state],
        )?;
        self.get(isolation_id)
    }

    pub(crate) fn delete(&self, isolation_id: &str, expected_state: &str) -> Result<bool, String> {
        self.write(
            "DELETE FROM isolated_dirs WHERE id = ?1 AND state = ?2",
            rusqlite::params![isolation_id, expected_state],
        )
    }
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<Record> {
    Ok(Record {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        repo_common_dir: row.get(2)?,
        main_root: row.get(3)?,
        worktree_path: row.get(4)?,
        worktree_git_dir: row.get(5)?,
        branch_ref: row.get(6)?,
        base_oid: row.get(7)?,
        state: row.get(8)?,
        usage_generation: row.get(9)?,
        attention: row.get(10)?,
        confirmation_digest: row.get(11)?,
        delete_oid: row.get(12)?,
    })
}
