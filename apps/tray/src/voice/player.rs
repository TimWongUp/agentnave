//! The office alone plays voice. The menu bar panel shows what it plays and what it listens to:
//! the office publishes that, the panel draws it and sends its clicks back to the office.

use crate::data::Response;
use serde_json::{Value, json};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

const OFFICE: &str = "main";
const PANEL: &str = "panel";

struct Player {
    /// What the office shows now, for a panel page that loads after it was published.
    shown: Option<Value>,
    /// The office numbers what it publishes; an older one arriving late is dropped.
    seq: u64,
    /// A click that waits for the office to open.
    waiting: Option<Value>,
}

static PLAYER: Mutex<Player> = Mutex::new(Player {
    shown: None,
    seq: 0,
    waiting: None,
});

pub fn request(
    app: &AppHandle,
    label: &str,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    match (method, path) {
        ("GET", "/app/voice/player") => {
            Response::json(&PLAYER.lock().unwrap().shown.clone().unwrap_or(Value::Null))
        }
        _ if method != "POST" => Response::error(405, "此操作只接受 POST"),
        _ if action != Some("1") => Response::error(403, "缺少操作请求头"),
        ("POST", "/app/voice/player") if label == OFFICE => match serde_json::from_slice(body) {
            Ok(shown) => {
                published(app, shown);
                Response::json(&json!({"ok": true}))
            }
            Err(_) => Response::error(400, "播放状态格式不对"),
        },
        ("POST", "/app/voice/player/command") if label == PANEL => command(app, body),
        _ => Response::error(403, "这个窗口不能操作语音播放"),
    }
}

/// The office page went away (another page, or its window closed): nothing plays any more.
pub fn clear(app: &AppHandle) {
    let mut player = PLAYER.lock().unwrap();
    player.shown = None;
    player.seq = 0;
    drop(player);
    tell_panel(app, &Value::Null);
}

// An office with its settings frame open still has the same top-level office URL.
fn office_showing(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    app.get_webview_window(OFFICE).filter(|office| {
        office
            .url()
            .is_ok_and(|current| current.path().starts_with("/chat/"))
    })
}

fn published(app: &AppHandle, shown: Value) {
    // A page that is leaving the office may still send one; only the office page counts.
    let Some(office) = office_showing(app) else {
        return;
    };
    let seq = shown["seq"].as_u64().unwrap_or(0);
    let mut player = PLAYER.lock().unwrap();
    if seq <= player.seq {
        return;
    }
    player.seq = seq;
    player.shown = Some(shown.clone());
    let waiting = player.waiting.take();
    drop(player);
    tell_panel(app, &shown);
    if let Some(click) = waiting {
        let _ = office.eval(event("agentnave:voice-command", &click));
    }
}

fn tell_panel(app: &AppHandle, shown: &Value) {
    if let Some(panel) = app.get_webview_window(PANEL) {
        let _ = panel.eval(event("agentnave:voice-player", shown));
    }
}

fn command(app: &AppHandle, body: &[u8]) -> Response {
    let Ok(click) = serde_json::from_slice::<Value>(body) else {
        return Response::error(400, "操作格式不对");
    };
    let kind = click["kind"].as_str().unwrap_or_default();
    if !matches!(
        kind,
        "toggle"
            | "previous"
            | "next"
            | "stop_all"
            | "flip"
            | "retry"
            | "settings"
            | "open"
            | "locate"
            | "listen"
            | "unlisten"
    ) {
        return Response::error(400, "未知的语音操作");
    }
    if kind == "settings" {
        crate::hide_panel(app);
        crate::open(app, OFFICE, "/settings/#voice-settings");
        return Response::json(&json!({"ok": true}));
    }
    // Until the office has published, its page cannot take a click yet.
    let ready = PLAYER.lock().unwrap().shown.is_some();
    let Some(office) = office_showing(app).filter(|_| ready) else {
        // Listening opens the office out of sight and starts once it has published.
        match kind {
            "listen" => {
                PLAYER.lock().unwrap().waiting = Some(click);
                let opening = crate::open_office_hidden(app);
                return Response::json(&json!({"ok": true, "opening": opening}));
            }
            // Stopping before the office opened just forgets the click.
            "unlisten" | "stop_all" => {
                PLAYER.lock().unwrap().waiting = None;
                return Response::json(&json!({"ok": true}));
            }
            _ => {}
        }
        return Response::error(404, "办公室没有打开");
    };
    // Opening a conversation shows it in the office, which goes there in place and keeps playing.
    if matches!(kind, "open" | "locate") {
        crate::hide_panel(app);
        crate::restore(app);
    }
    let _ = office.eval(event("agentnave:voice-command", &click));
    Response::json(&json!({"ok": true}))
}

fn event(name: &str, detail: &Value) -> String {
    format!("window.dispatchEvent(new CustomEvent({name:?}, {{detail: {detail}}}));")
}
