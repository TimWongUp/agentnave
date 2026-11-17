//! The App owns host cleanup and user consent; the OS uninstaller only removes program files.
#[cfg(any(windows, test))]
mod channel;
mod cleanup;
#[cfg(windows)]
mod windows;

use crate::data::snapshot::Overview;
use crate::response::Response;
use crate::{Reader, preferences};
use serde_json::{Value, json};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

#[derive(Default)]
pub(crate) struct Uninstall {
    operation: Mutex<()>,
    session: Mutex<Session>,
}

#[derive(Default)]
struct Session {
    #[cfg(windows)]
    reply: Option<channel::Reply>,
    #[cfg(windows)]
    started_for_uninstall: bool,
    confirmed: bool,
}

pub(crate) fn entry() {
    #[cfg(windows)]
    if let Some(code) = windows::entry() {
        std::process::exit(code);
    }
}

pub(crate) fn forwarded(app: &AppHandle, args: &[String], startup: bool) -> bool {
    #[cfg(windows)]
    if let Some(argument) = args.windows(2).find(|args| args[0] == channel::REQUEST) {
        if crate::PREVIEW {
            return true;
        }
        let Ok(reply) = channel::Reply::connect(&argument[1]) else {
            return true;
        };
        let state = app.state::<Uninstall>();
        let mut session = state.session.lock().unwrap_or_else(|p| p.into_inner());
        if session.reply.is_some() || session.confirmed {
            reply.finish(false);
            return true;
        }
        session.reply = Some(reply);
        session.started_for_uninstall = startup;
        drop(session);
        crate::window::open(app, crate::TRAY, "/settings/?uninstall#about");
        return true;
    }
    let _ = (app, args, startup);
    false
}

/// Also called on normal quit/window shutdown: no answer or a crashed App is never consent.
pub(crate) fn exited(app: &AppHandle) {
    let state = app.state::<Uninstall>();
    let mut session = state.session.lock().unwrap_or_else(|p| p.into_inner());
    #[cfg(windows)]
    if let Some(reply) = session.reply.take() {
        reply.finish(session.confirmed);
    }
    #[cfg(not(windows))]
    let _ = &mut session;
}

fn guard(preview: bool, overview: &Overview) -> Result<(), String> {
    if preview {
        return Err("预览版不能卸载 AgentNave".into());
    }
    if overview.count("running") + overview.count("review") > 0 {
        return Err("还有任务在跑，结束之后才能卸载".into());
    }
    if overview.running_uncertain() {
        return Err("无法确认运行状态，请恢复数据读取后重试".into());
    }
    Ok(())
}

pub(crate) fn request(
    app: &AppHandle,
    reader: &Reader,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    if method == "GET" && path == "/app/uninstall" {
        let error = guard(crate::PREVIEW, &reader.overview()).err();
        return Response::json(&json!({"blocked": error, "ready": reader.uninstall_ready()}));
    }
    if path == "/app/uninstall/cancel" {
        return crate::response::action(method, action, || {
            cancel(app);
            Response::json(&json!({"ok": true}))
        });
    }
    if path != "/app/uninstall" {
        return Response::not_found();
    }
    crate::response::post(method, action, body, |payload| {
        match execute(app, reader, &payload) {
            Ok(failures) if failures.is_empty() => Response::json(&json!({"ok": true})),
            Ok(failures) => Response::json(&json!({"ok": false, "failures": failures})),
            Err(error) => Response::error(409, &error),
        }
    })
}

pub(crate) fn cancel(app: &AppHandle) {
    let state = app.state::<Uninstall>();
    let Ok(_operation) = state.operation.try_lock() else {
        return;
    };
    #[cfg(windows)]
    {
        let mut session = state.session.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(reply) = session.reply.take() {
            reply.finish(false);
        }
        if session.started_for_uninstall && !session.confirmed {
            app.exit(1);
        }
    }
}

fn execute(app: &AppHandle, reader: &Reader, payload: &Value) -> Result<Vec<Value>, String> {
    let state = app.state::<Uninstall>();
    let _operation = state.operation.try_lock().map_err(|_| "卸载正在进行")?;
    guard(crate::PREVIEW, &reader.overview())?;
    let delete = payload["delete_history"]
        .as_bool()
        .ok_or("缺少记录删除选项")?;
    let home = dirs::home_dir().ok_or("无法定位用户目录")?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let plan = cleanup::Plan::new(
        vec![
            app.path().app_data_dir().map_err(|e| e.to_string())?,
            app.path().app_local_data_dir().map_err(|e| e.to_string())?,
            preferences::dir(),
        ],
        reader.data_root(),
        delete,
        &home,
        &executable,
    )?;
    // An installer/upgrade already claimed under this lock must finish before removal starts;
    // no new update can claim it afterwards, including startup's missing-service installer.
    app.state::<crate::Updates>().begin_uninstall()?;
    let mut failures = reader.uninstall_hosts()?;
    if !failures.is_empty() {
        return Ok(failures);
    }
    if let Err(error) = preferences::autostart::remove(app) {
        failures.push(json!({"host": "开机启动", "error": error}));
        return Ok(failures);
    }
    #[cfg(windows)]
    {
        let external = state
            .session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .reply
            .is_some();
        windows::finish(&plan, external)?;
    }
    #[cfg(unix)]
    cleanup::spawn(&plan)?;
    state
        .session
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .confirmed = true;
    // No more writes are accepted; Exit saves window state before notifying the waiter.
    app.exit(0);
    Ok(Vec::new())
}

#[cfg(test)]
mod tests;
