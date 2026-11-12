//! Native integration for App preferences; all writes pass here before touching the system.
use std::sync::Mutex;

use serde_json::{Value, json};
use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Manager, Wry};

use super::{autostart, general, keep_awake};
use crate::data::snapshot::Overview;
use crate::response::Response;

#[derive(Default)]
pub struct Runtime {
    writes: Mutex<()>,
    menu: Mutex<Vec<MenuItem<Wry>>>,
    awake: keep_awake::Guard,
    scale: Mutex<u64>,
}

pub fn state(app: &AppHandle) -> Value {
    let mut state = super::state(&super::dir());
    let enabled = match autostart::is_enabled(app) {
        Ok(enabled) => enabled,
        Err(error) => {
            state["error"] = json!(format!("无法读取开机启动状态：{error}"));
            false
        }
    };
    let scale = *app.state::<Runtime>().scale.lock().unwrap();
    response(state, enabled, scale, crate::PREVIEW)
}

pub(super) fn response(mut state: Value, enabled: bool, scale: u64, preview: bool) -> Value {
    let mut effective = general::effective(&state["preferences"], enabled);
    effective["scale"] = json!(if scale == 0 { 100 } else { scale });
    // Backfill only the response. Reading never rewrites the preferences file.
    if !state["preferences"]["general"].is_object() {
        state["preferences"]["general"] = json!({});
    }
    state["preferences"]["general"]["autostart"] = json!(enabled);
    state["effective"] = json!({"general": effective});
    state["autostart_available"] = json!(!preview);
    state
}

pub fn request(app: &AppHandle, method: &str, action: Option<&str>, body: &[u8]) -> Response {
    if method == "GET" {
        return Response::json(&state(app));
    }
    if method != "POST" {
        return Response::error(405, "此路由只接受 GET 或 POST");
    }
    if action != Some("1") {
        return Response::error(403, "缺少操作请求头");
    }
    let payload: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return Response::error(400, "无效的 JSON"),
    };
    let result = save(app, &payload);
    match result {
        Ok(document) => Response::json(&json!({"saved": true, "preferences": document})),
        Err(error) => Response::error(400, &error),
    }
}

pub fn save(app: &AppHandle, payload: &Value) -> Result<Value, String> {
    let runtime = app.state::<Runtime>();
    let _write = runtime.writes.lock().unwrap();
    super::validate_changes(payload)?;
    let requested = payload["general"]["autostart"].as_bool();
    let previous = if requested.is_some() {
        Some(autostart::is_enabled(app)?)
    } else {
        None
    };
    if let Some(enabled) = requested {
        autostart::set(app, enabled)?;
    }
    let document = match super::update(&super::dir(), payload) {
        Ok(document) => document,
        Err(error) => {
            if let Some(previous) = previous
                && let Err(rollback) = autostart::set(app, previous)
            {
                return Err(format!("{error}；恢复开机启动状态失败：{rollback}"));
            }
            return Err(error);
        }
    };
    apply(app).map_err(|error| format!("设置已保存，但应用失败：{error}"))?;
    Ok(document)
}

pub fn apply(app: &AppHandle) -> Result<(), String> {
    let mut general = general::current();
    general["autostart"] = json!(autostart::is_enabled(app)?);
    app.set_theme(match general["theme"].as_str() {
        Some("light") => Some(tauri::Theme::Light),
        Some("dark") => Some(tauri::Theme::Dark),
        _ => None,
    });
    if let Some(window) = app.get_webview_window("main") {
        zoom(&window)?;
    } else {
        *app.state::<Runtime>().scale.lock().unwrap() = general["scale"].as_u64().unwrap_or(100);
    }
    let script = format!(
        "window.dispatchEvent(new CustomEvent('agentnave:preferences', {{detail: {general}}}));"
    );
    for window in app.webview_windows().values() {
        let _ = window.eval(&script);
    }
    let runtime = app.state::<Runtime>();
    for item in runtime.menu.lock().unwrap().iter() {
        let (zh, en) = match item.id().as_ref() {
            "open" => ("打开 AgentNave", "Open AgentNave"),
            "settings" => ("设置…", "Settings…"),
            "quit" => ("退出 AgentNave", "Quit AgentNave"),
            _ => continue,
        };
        let _ = item.set_text(general::text(zh, en));
    }
    if let Some(updates) = app.try_state::<crate::Updates>() {
        updates.refresh_label();
    }
    refresh_working(app, &app.state::<crate::Tray>().reader.overview())
}

pub fn zoom(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Ok(());
    }
    let scale = general::current()["scale"].as_u64().unwrap_or(100);
    window
        .set_zoom(scale as f64 / 100.0)
        .map_err(|error| error.to_string())?;
    *window.app_handle().state::<Runtime>().scale.lock().unwrap() = scale;
    Ok(())
}

pub fn refresh_working(app: &AppHandle, overview: &Overview) -> Result<(), String> {
    crate::badge::show(
        app,
        crate::badge::count(overview, general::current),
        general::current,
    );
    let keep = general::current()["keep_awake"] == true;
    // A transient SQLite/view read failure reports zero running calls; keep the current
    // assertion until a clean scan confirms work ended, unless the user turned keep_awake off.
    if keep && overview.count("running") == 0 && overview.running_uncertain() {
        return Ok(());
    }
    app.state::<Runtime>()
        .awake
        .set(keep && overview.count("running") > 0)
}

pub fn shutdown(app: &AppHandle) {
    app.state::<Runtime>().awake.shutdown();
}

pub fn menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let open = MenuItem::with_id(
        app,
        "open",
        general::text("打开 AgentNave", "Open AgentNave"),
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(
        app,
        "settings",
        general::text("设置…", "Settings…"),
        true,
        None::<&str>,
    )?;
    let check = MenuItem::with_id(
        app,
        "update",
        general::text("检查更新…", "Check for Updates…"),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        general::text("退出 AgentNave", "Quit AgentNave"),
        true,
        None::<&str>,
    )?;
    let menu = if crate::PREVIEW {
        Menu::with_items(app, &[&open, &settings, &quit])?
    } else {
        Menu::with_items(app, &[&open, &settings, &check, &quit])?
    };
    app.manage(crate::Updates::new(check));
    *app.state::<Runtime>().menu.lock().unwrap() = vec![open, settings, quit];
    Ok(menu)
}
