//! Guards against resuming a child workspace whose owning derived session is still alive (ADR-0128 §5).

use std::path::Path;

use serde_json::json;

use super::{Terminals, thread_desk_key};
use crate::data::boss_app::boss_key;
use crate::response::Response;

fn workspace_owner(root: &Path, workspace: &str) -> String {
    let Some(connection) = crate::db::read(root).ok().flatten() else {
        return workspace.to_owned();
    };
    connection
        .query_row(
            "SELECT owner FROM workspaces WHERE id = ?1",
            [workspace],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .filter(|owner| !owner.is_empty())
        .unwrap_or_else(|| workspace.to_owned())
}

impl Terminals {
    /// If `workspace` is owned by a live derived terminal session, refuse opening a duplicate
    /// standalone process and point to that derived terminal instead.
    pub(super) fn reject_if_live_derived_owner(
        &self,
        root: &Path,
        workspace: &str,
    ) -> Option<Response> {
        let owner = workspace_owner(root, workspace);
        let state = self.lock();
        let live = state.open.iter().find(|r| {
            r.session.exit().is_none()
                && r.derived.is_some()
                && r.native
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .is_some_and(|sid| boss_key(&r.provider, sid) == owner)
        })?;
        let derived = live.derived.as_ref()?;
        let desk_key = live
            .native
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|sid| thread_desk_key(&live.provider, sid));
        let mut resp = Response::json(&json!({
            "error": "这段对话的派生终端正在运行，请在对应的派生终端里继续，不能另开第二个进程",
            "derived_terminal": {
                "id": live.id,
                "parent_workspace_id": derived.parent_workspace_id,
                "desk_key": desk_key,
            }
        }));
        resp.status = 409;
        Some(resp)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use portable_pty::CommandBuilder;
    use serde_json::Value;

    use super::super::{DerivedInfo, NewDerived, Terminals};
    use crate::data::boss_app::boss_key;
    use crate::terminal::session::Session;

    fn live_session() -> Arc<Session> {
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args([
            "-c",
            "stty -echo; printf up; while read -r _line; do :; done",
        ]);
        cmd.cwd(std::env::temp_dir());
        let s = Session::start(cmd, 80, 24).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !s.recent_screen_text(64).contains("up") {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        s
    }

    #[test]
    fn rejects_resume_when_owning_derived_terminal_is_alive() {
        let dir = tempfile::tempdir().unwrap();
        let root: &Path = dir.path();
        let db = crate::db::create(root);
        let sid = "33333333-3333-4333-8333-333333333333";
        let owner = boss_key("codex", sid);
        let child_ws = "s-child-ws";
        db.execute(
            "INSERT INTO workspaces (id, owner, owner_host) VALUES (?1, ?2, 'codex')",
            [child_ws, owner.as_str()],
        )
        .unwrap();

        let terminals = Terminals::default();
        assert!(
            terminals
                .reject_if_live_derived_owner(root, child_ws)
                .is_none()
        );

        let tid = terminals.register_derived(NewDerived {
            provider: "codex".to_owned(),
            cwd: root.to_path_buf(),
            session: live_session(),
            native: Some(sid.to_owned()),
            account: None,
            codex: None,
            info: DerivedInfo {
                parent_boss_key: "s-parent".to_owned(),
                parent_workspace_id: Some("s-boss-ws".to_owned()),
                busy: false,
                blocked_kind: None,
                isolation_id: None,
                _lease: None,
                _log_dir: None,
                _hook_dir: None,
                codex_ready_file: None,
                active_invocation_id: None,
                launch_config: None,
                life: Default::default(),
            },
        });
        let resp = terminals
            .reject_if_live_derived_owner(root, child_ws)
            .expect("must refuse resume while derived owner lives");
        assert_eq!(resp.status, 409);
        let body: Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(body["derived_terminal"]["id"], tid);
        assert_eq!(body["derived_terminal"]["parent_workspace_id"], "s-boss-ws");
    }
}
