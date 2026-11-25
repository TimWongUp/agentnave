//! Starts the CLI of a derived session whose process has ended again on its native session,
//! submitting nothing.

use super::super::Terminals;
use super::super::lifecycle::{Relaunched, WakePlan};
use serde_json::json;

use super::super::spawn_requests::RunConfig;
use super::claude_hook;
#[cfg(unix)]
use super::codex;
use super::tui::{self, TuiKind};

#[cfg_attr(not(unix), allow(unused_variables))]
pub(in crate::terminal) fn relaunch(
    terminals: &Terminals,
    plan: &WakePlan,
) -> Result<Relaunched, String> {
    match plan.provider.as_str() {
        "claude" => {
            let hook_dir = claude_hook::prepare_claude_hook(false)?;
            relaunch_tui(plan, TuiKind::Claude, Some(hook_dir))
        }
        "antigravity" => relaunch_tui(plan, TuiKind::Antigravity, None),
        #[cfg(unix)]
        "codex" => codex::relaunch_codex(terminals, plan),
        _ => Err("这个 CLI 的派生会话不能在这里恢复".to_owned()),
    }
}

fn relaunch_tui(
    plan: &WakePlan,
    kind: TuiKind,
    hook_dir: Option<tempfile::TempDir>,
) -> Result<Relaunched, String> {
    let cfg = wake_config(plan)?;
    let started = tui::launch_tui(
        &cfg,
        Some(&plan.native),
        None,
        &plan.cwd,
        plan.account.as_ref(),
        kind,
        hook_dir.as_ref().map(tempfile::TempDir::path),
    )?;
    Ok(Relaunched {
        session: started.session,
        log_dir: started.log_dir,
        hook_dir,
        #[cfg(unix)]
        codex: None,
    })
}

/// The launch the session began with, in the permission mode it ended up in: without a prompt to
/// check the mode by, there is no falling back this time. Without one, the CLI's own settings.
pub(super) fn wake_config(plan: &WakePlan) -> Result<RunConfig, String> {
    let config = plan.config.as_ref();
    let mut permissions = config.map_or_else(|| json!({}), |c| c.permissions.clone());
    if let Some(options) = permissions.as_object_mut() {
        options.remove("permission_fallback");
        if let Some(mode) = &plan.fallback_mode {
            options.insert("permission_mode".to_owned(), mode.as_str().into());
        }
    }
    Ok(RunConfig {
        provider: plan.provider.clone(),
        account: None,
        account_dir: None,
        cwd: plan.cwd.display().to_string(),
        isolation: None,
        model: config.and_then(|c| c.model.clone()),
        effort: config.and_then(|c| c.effort.clone()),
        permissions,
    })
}
