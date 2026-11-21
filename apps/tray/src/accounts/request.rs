//! The accounts page's routes: list, sign-in state, create, rename, sign in and remove. Every
//! write answers with the accounts as they are afterwards.

use super::{Account, auth, remove};
use crate::hosts::Hosts;
use crate::response::{self, Response};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::AppHandle;

/// What a request needs from the App around it.
pub(crate) struct Context<'a> {
    pub root: &'a Path,
    pub home: PathBuf,
    pub hosts: &'a Mutex<Option<Hosts>>,
    /// Whether no task or meeting runs, or why one does.
    pub idle: &'a dyn Fn() -> Result<(), String>,
}

pub(crate) fn request(
    ctx: &Context,
    app: Option<&AppHandle>,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    match (method, route) {
        ("GET", "/app/accounts") => Response::json(&super::state(ctx.root)),
        ("GET", "/app/accounts/status") => status(ctx, query),
        (_, "/app/accounts/create") => {
            response::post(method, action, body, |payload| create(ctx, &payload))
        }
        (_, "/app/accounts/rename") => {
            response::post(method, action, body, |payload| rename(ctx, &payload))
        }
        (_, "/app/accounts/login") => {
            response::post(method, action, body, |payload| login(ctx, app, &payload))
        }
        (_, "/app/accounts/remove") => {
            response::post(method, action, body, |payload| remove(ctx, app, &payload))
        }
        _ => Response::not_found(),
    }
}

fn param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

/// The default account's folder: the `claude` row's, which follows the environment.
fn default_dir(ctx: &Context) -> PathBuf {
    crate::hosts::default_claude_dir(ctx.hosts).unwrap_or_else(|| {
        std::env::var_os(auth::CONFIG_VARIABLE)
            .filter(|dir| !dir.is_empty())
            .map_or_else(|| ctx.home.join(".claude"), PathBuf::from)
    })
}

fn registered(ctx: &Context, id: &str) -> Result<Account, Response> {
    let accounts = super::read(ctx.root).map_err(|error| Response::error(409, &error))?;
    accounts
        .into_iter()
        .find(|account| account.id == id)
        .ok_or_else(|| Response::error(404, "找不到这个账号"))
}

fn status(ctx: &Context, query: &str) -> Response {
    let state = match param(query, "id") {
        Some("default") => {
            // Only a folder the environment named is passed on; the usual one is Claude's own pick.
            let dir = default_dir(ctx);
            auth::status((dir != ctx.home.join(".claude")).then_some(dir.as_path()))
        }
        Some(id) => match registered(ctx, id) {
            Ok(account) => auth::status(Some(&account.dir)),
            Err(reply) => return reply,
        },
        None => return Response::error(400, "缺少账号"),
    };
    Response::json(&state)
}

/// The sync's outcome as the page tells it; the account is registered either way.
fn sync(ctx: &Context, id: &str) -> Value {
    match crate::hosts::sync_account(ctx.root, ctx.hosts, id) {
        Ok(message) => json!({"ok": true, "message": message}),
        Err(message) => json!({"ok": false, "message": message}),
    }
}

fn create(ctx: &Context, payload: &Value) -> Response {
    let label = payload["label"].as_str().unwrap_or_default();
    let account = match super::create(ctx.root, &ctx.home, label) {
        Ok(account) => account,
        Err(error) => return Response::error(400, &error),
    };
    let sync = sync(ctx, &account.id);
    Response::json(&json!({
        "account": account.value(), "sync": sync, "state": super::state(ctx.root)
    }))
}

fn rename(ctx: &Context, payload: &Value) -> Response {
    let id = payload["id"].as_str().unwrap_or_default();
    let label = payload["label"].as_str().unwrap_or_default();
    match super::rename(ctx.root, id, label) {
        Ok(_) => Response::json(&json!({"state": super::state(ctx.root)})),
        Err(error) => Response::error(400, &error),
    }
}

fn login(ctx: &Context, app: Option<&AppHandle>, payload: &Value) -> Response {
    let Some(app) = app else {
        return Response::error(500, "无法打开终端");
    };
    let id = payload["id"].as_str().unwrap_or_default();
    let (account_id, dir) = if id == "default" {
        ("default".to_owned(), None)
    } else {
        match registered(ctx, id) {
            Ok(account) => (account.id, Some(account.dir)),
            Err(reply) => return reply,
        }
    };
    let Some(program) = auth::program() else {
        return Response::error(400, "这个 CLI 没有安装");
    };
    let size = (payload["cols"].as_u64(), payload["rows"].as_u64());
    match crate::terminal::open_login(app, &account_id, dir.as_deref(), &program, &ctx.home, size) {
        Ok(session) => Response::json(&json!({"session": session})),
        Err(error) => Response::error(500, &error),
    }
}

fn remove(ctx: &Context, app: Option<&AppHandle>, payload: &Value) -> Response {
    let account = match registered(ctx, payload["id"].as_str().unwrap_or_default()) {
        Ok(account) => account,
        Err(reply) => return reply,
    };
    let delete = payload["delete_dir"].as_bool().unwrap_or(false);
    if delete {
        // Everything that could still be using the folder is checked before anything changes.
        if let Err(error) = remove::deletable(&account, &ctx.home).and_then(|()| (ctx.idle)()) {
            return Response::error(409, &error);
        }
        if app.is_some_and(|app| crate::terminal::account_in_use(app, &account.id)) {
            return Response::error(409, "这个账号还有打开的终端，关闭后才能删除目录");
        }
    }
    // The connection leaves the folder while the account still names it; a folder that cannot be
    // cleaned does not keep the account from being unbound.
    let unsync = crate::hosts::unsync_account(ctx.root, ctx.hosts, &account.id).err();
    if let Err(error) = super::unbind(ctx.root, &account.id) {
        return Response::error(500, &error);
    }
    let deleted = delete.then(|| remove::delete(&account, &ctx.home));
    Response::json(&json!({
        "state": super::state(ctx.root),
        "deleted": deleted.as_ref().is_some_and(Result::is_ok),
        "delete_error": deleted.and_then(Result::err),
        "unsync_error": unsync,
    }))
}
