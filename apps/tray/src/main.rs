//! AgentNave tray: view and clean up this machine's saved Studio conversations.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod avatars;
mod characters;
mod data;
mod db;
mod detect;
mod hosts;
mod manage;
mod members;
mod onboarding;
mod rules;
mod settings;
mod stage;
mod update;

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tauri::http::{Response, header};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::webview::NewWindowResponse;
use tauri::{
    AppHandle, LogicalPosition, Manager, Rect, RunEvent, Url, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use data::{CSP, Overview, Reader};
use update::Updates;

const SCHEME: &str = "agentnave";
const TRAY: &str = "main";
const PANEL: &str = "panel";
const PANEL_SIZE: (f64, f64) = (480.0, 640.0);
const MAIN_SIZE: (f64, f64) = (1440.0, 900.0);
const MAIN_MIN: (f64, f64) = (420.0, 480.0);
const POLL: Duration = Duration::from_secs(2);
/// A click on the tray icon first blurs the open panel; this keeps that click from reopening it.
const REOPEN_GUARD: Duration = Duration::from_millis(300);
const DRIP_FRAME: Duration = Duration::from_millis(33);
/// A click lets the icon's dot fall onto the N like a drop and spring back (icons/drip.py
/// draws the frames; the last one is the rest pose).
const DRIP: [&[u8]; 18] = [
    include_bytes!("../icons/drip/00.png"),
    include_bytes!("../icons/drip/01.png"),
    include_bytes!("../icons/drip/02.png"),
    include_bytes!("../icons/drip/03.png"),
    include_bytes!("../icons/drip/04.png"),
    include_bytes!("../icons/drip/05.png"),
    include_bytes!("../icons/drip/06.png"),
    include_bytes!("../icons/drip/07.png"),
    include_bytes!("../icons/drip/08.png"),
    include_bytes!("../icons/drip/09.png"),
    include_bytes!("../icons/drip/10.png"),
    include_bytes!("../icons/drip/11.png"),
    include_bytes!("../icons/drip/12.png"),
    include_bytes!("../icons/drip/13.png"),
    include_bytes!("../icons/drip/14.png"),
    include_bytes!("../icons/drip/15.png"),
    include_bytes!("../icons/drip/16.png"),
    include_bytes!("../icons/drip/17.png"),
];
static DRIPPING: AtomicBool = AtomicBool::new(false);
/// A preview build (`scripts/preview_tray.sh`) runs beside the installed app on the same data to
/// show UI changes: it never changes AgentNave data, host configs or Skills, and never updates or
/// installs. Only its own WebView storage (display settings) is kept, under its own identifier.
pub const PREVIEW: bool = option_env!("AGENTNAVE_PREVIEW").is_some();

/// What a preview build refuses: every request that could change files, except moving its window.
fn preview_refuses(preview: bool, method: &str, path: &str) -> bool {
    preview && method != "GET" && !matches!(path, "/app/window/drag" | "/app/window/zoom")
}

struct Tray {
    reader: Arc<Reader>,
    panel_hidden: Mutex<Option<Instant>>,
}

fn main() {
    let reader = Arc::new(Reader::new(data::data_root()));
    let served = reader.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| restore(app)))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .register_asynchronous_uri_scheme_protocol(SCHEME, move |ctx, request, responder| {
            let reader = served.clone();
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_owned();
            let label = ctx.webview_label().to_owned();
            let flag = |name: &str| {
                request
                    .uri()
                    .query()
                    .is_some_and(|query| query.split('&').any(|pair| pair == name))
            };
            let refresh = flag("refresh=1");
            let quiet = flag("quiet=1");
            thread::spawn(move || {
                let method = request.method().as_str();
                let action = request
                    .headers()
                    .get("X-AgentNave-Action")
                    .and_then(|v| v.to_str().ok());
                let reply = match (method, path.as_str()) {
                    _ if preview_refuses(PREVIEW, method, &path) => {
                        data::Response::error(403, "预览版只读：请在正式版里保存改动")
                    }
                    ("GET", "/app/status") => data::Response::json(&update::status(&app)),
                    // The top bar moves and zooms its window, as a native title bar would.
                    ("POST", "/app/window/drag" | "/app/window/zoom") if action == Some("1") => {
                        match app.get_webview_window(&label) {
                            Some(window) if path.ends_with("drag") => {
                                let _ = window.start_dragging();
                                data::Response::json(&serde_json::json!({"ok": true}))
                            }
                            Some(window) => {
                                let _ = if window.is_maximized().unwrap_or(false) {
                                    window.unmaximize()
                                } else {
                                    window.maximize()
                                };
                                data::Response::json(&serde_json::json!({"ok": true}))
                            }
                            None => data::Response::error(404, "找不到窗口"),
                        }
                    }
                    ("GET", "/settings/members") => reader.members(refresh),
                    (_, "/app/service/install") if method != "POST" => {
                        data::Response::error(405, "此操作只接受 POST")
                    }
                    (_, "/app/service/install") if action != Some("1") => {
                        data::Response::error(403, "缺少操作请求头")
                    }
                    (_, "/app/service/install") => {
                        data::Response::json(&update::install_service_now(&app))
                    }
                    (_, "/app/update/check" | "/app/update/apply") if method != "POST" => {
                        data::Response::error(405, "此操作只接受 POST")
                    }
                    (_, "/app/update/check" | "/app/update/apply") if action != Some("1") => {
                        data::Response::error(403, "缺少操作请求头")
                    }
                    (_, "/app/update/check") => {
                        data::Response::json(&update::check_now(&app, !quiet))
                    }
                    (_, "/app/update/apply") => data::Response::json(&update::apply(&app)),
                    _ => reader.request(method, &path, action, request.body()),
                };
                let response = Response::builder()
                    .status(reply.status)
                    .header(header::CONTENT_TYPE, reply.content_type)
                    .header(header::CACHE_CONTROL, reply.cache_control)
                    .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                    .header(header::REFERRER_POLICY, "no-referrer")
                    .header(header::CONTENT_SECURITY_POLICY, CSP)
                    .body(reply.body)
                    .expect("static response headers are valid");
                responder.respond(response);
            });
        })
        .manage(Tray {
            reader: reader.clone(),
            panel_hidden: Mutex::new(None),
        })
        .setup(move |app| {
            // A regular app: it has a Dock icon and can be kept in the Dock; the menu bar icon stays.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);
            let handle = app.handle();
            let host_context = hosts::Context::live(handle.path().app_data_dir()?);
            let takeover = reader.clone();
            build_panel(handle)?;
            let overview = reader.overview();
            // Left click opens the panel; right click shows this short menu.
            let check = MenuItem::with_id(handle, "update", "检查更新…", true, None::<&str>)?;
            let open_item =
                MenuItem::with_id(handle, "open", "打开 AgentNave", true, None::<&str>)?;
            let settings = MenuItem::with_id(handle, "settings", "设置…", true, None::<&str>)?;
            let quit = MenuItem::with_id(handle, "quit", "退出 AgentNave", true, None::<&str>)?;
            // A preview build never updates itself, so it offers no update check.
            let menu = if PREVIEW {
                Menu::with_items(handle, &[&open_item, &settings, &quit])?
            } else {
                Menu::with_items(handle, &[&open_item, &settings, &check, &quit])?
            };
            app.manage(Updates::new(check));
            TrayIconBuilder::with_id(TRAY)
                .icon(Image::from_bytes(include_bytes!("../icons/tray@2x.png"))?)
                .icon_as_template(true)
                .tooltip(headline(&overview))
                .title(running_badge(&overview))
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => restore(app),
                    "settings" => open(app, "main", "/settings/"),
                    "update" if !PREVIEW => update::act(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        drip(tray);
                        toggle_panel(tray.app_handle(), rect);
                    }
                })
                .build(app)?;
            let handle = handle.clone();
            let checker = handle.clone();
            let resumer = handle.clone();
            thread::spawn(move || {
                let shell = takeover.probe_tools(&host_context);
                thread::spawn(move || {
                    takeover.configure_hosts(host_context, shell);
                    // Refreshing repairs host configs and reinstalls Skills; that stays with the installed app.
                    if !PREVIEW {
                        takeover.refresh_hosts();
                    }
                });
                if !PREVIEW {
                    update::resume(&resumer);
                }
            });
            thread::spawn(members::prepare);
            thread::spawn(move || poll(handle, reader, overview));
            if !PREVIEW {
                thread::spawn(move || {
                    loop {
                        update::check(&checker, false);
                        thread::sleep(update::EVERY);
                    }
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start the AgentNave tray");
    app.run(|app, event| match event {
        // Closing the last window keeps the tray running; only "退出" ends the app.
        RunEvent::ExitRequested {
            api, code: None, ..
        } => api.prevent_exit(),
        // Like any regular app, launching it shows the main window. Opening it once the event
        // loop runs keeps its first requests from stalling behind start-up.
        RunEvent::Ready => open(app, "main", "/chat/"),
        RunEvent::Exit => {
            if let Some(window) = app.get_webview_window("main") {
                save_size(&window);
            }
        }
        // A click on the Dock icon brings the main window back where it was left.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => restore(app),
        _ => {}
    });
}

fn drip(tray: &TrayIcon) {
    if DRIPPING.swap(true, Ordering::AcqRel) {
        return;
    }
    let tray = tray.clone();
    thread::spawn(move || {
        for frame in DRIP {
            if let Ok(image) = Image::from_bytes(frame) {
                let _ = tray.set_icon_with_as_template(Some(image), true);
            }
            thread::sleep(DRIP_FRAME);
        }
        DRIPPING.store(false, Ordering::Release);
    });
}

fn poll(app: AppHandle, reader: Arc<Reader>, mut shown: Overview) {
    loop {
        thread::sleep(POLL);
        let overview = reader.overview();
        if overview == shown {
            continue;
        }
        if let Some(tray) = app.tray_by_id(TRAY) {
            let _ = tray.set_tooltip(Some(headline(&overview)));
            let _ = tray.set_title(Some(running_badge(&overview)));
        }
        shown = overview;
    }
}

fn headline(overview: &Overview) -> String {
    if overview.missing {
        return "AgentNave · 尚未找到工作室数据".to_owned();
    }
    format!("AgentNave · 运行中 {}", overview.count("running"))
}

/// Shown beside the menu bar icon: how many subagents are running, nothing when idle; a preview
/// build always says so, to tell its icon from the installed app's.
fn running_badge(overview: &Overview) -> String {
    let count = match overview.count("running") {
        0 => String::new(),
        n => format!("={n}"),
    };
    if PREVIEW {
        format!("预览{count}")
    } else {
        count
    }
}

fn url(path: &str) -> Url {
    let base = if cfg!(any(windows, target_os = "android")) {
        format!("http://{SCHEME}.localhost")
    } else {
        format!("{SCHEME}://localhost")
    };
    Url::parse(&base)
        .and_then(|base| base.join(path))
        .expect("tray routes are valid paths")
}

fn ours(target: &Url) -> bool {
    let base = url("/");
    target.scheme() == base.scheme() && target.host_str() == base.host_str()
}

/// The menu bar panel: created hidden at start, so a click shows it at once.
fn build_panel(app: &AppHandle) -> tauri::Result<()> {
    let glass = cfg!(target_os = "macos");
    let handle = app.clone();
    let builder = WebviewWindowBuilder::new(
        app,
        PANEL,
        WebviewUrl::CustomProtocol(url(if glass { "/panel/?glass" } else { "/panel/" })),
    )
    .title("AgentNave")
    .inner_size(PANEL_SIZE.0, PANEL_SIZE.1)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .on_navigation(move |target| {
        // Panel links name workbench routes or app actions; none navigates the panel itself.
        if !ours(target) || target.path().starts_with("/panel/") {
            return ours(target);
        }
        let (app, path) = (handle.clone(), target.path().to_owned());
        let destination = destination(target);
        thread::spawn(move || match path.as_str() {
            // The panel stays open: its notice shows the progress or "已复制".
            "/app/update" | "/app/upgrade-service" if PREVIEW => {}
            "/app/update" => update::act(&app),
            "/app/upgrade-service" => update::upgrade_service(&app),
            "/app/copy-service-command" => update::copy_service_command(&app),
            _ => {
                if let Some(panel) = app.get_webview_window(PANEL) {
                    let _ = panel.hide();
                }
                if path == "/app/quit" {
                    app.exit(0);
                } else {
                    open(&app, "main", &destination);
                }
            }
        });
        false
    });
    #[cfg(target_os = "macos")]
    let builder = builder.transparent(true).effects(
        tauri::window::EffectsBuilder::new()
            .effect(tauri::window::Effect::Popover)
            .state(tauri::window::EffectState::Active)
            .radius(12.0)
            .build(),
    );
    let panel = builder.build()?;
    let app = app.clone();
    panel.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event
            && let Some(panel) = app.get_webview_window(PANEL)
        {
            let _ = panel.hide();
            *app.state::<Tray>().panel_hidden.lock().unwrap() = Some(Instant::now());
        }
    });
    Ok(())
}

fn toggle_panel(app: &AppHandle, icon: Rect) {
    let Some(panel) = app.get_webview_window(PANEL) else {
        return;
    };
    if panel.is_visible().unwrap_or(false) {
        let _ = panel.hide();
        return;
    }
    let hidden = *app.state::<Tray>().panel_hidden.lock().unwrap();
    if hidden.is_some_and(|at| at.elapsed() < REOPEN_GUARD) {
        return;
    }
    place(&panel, icon);
    let _ = panel.show();
    let _ = panel.set_focus();
}

/// Center the panel under the icon (above it when the taskbar is at the bottom), on screen.
fn place(panel: &WebviewWindow, icon: Rect) {
    // Work in logical points: the icon rect is physical at its own screen's scale, and
    // monitors with different scales cannot share one physical coordinate space.
    let screen = panel
        .available_monitors()
        .unwrap_or_default()
        .into_iter()
        .find_map(|monitor| {
            let scale = monitor.scale_factor();
            let point = icon.position.to_logical::<f64>(scale);
            let origin = monitor.position().to_logical::<f64>(scale);
            let size = monitor.size().to_logical::<f64>(scale);
            (point.x >= origin.x
                && point.x < origin.x + size.width
                && point.y >= origin.y
                && point.y < origin.y + size.height)
                .then_some((scale, origin, size))
        });
    let scale = screen.map_or_else(|| panel.scale_factor().unwrap_or(1.0), |(s, _, _)| s);
    let at = icon.position.to_logical::<f64>(scale);
    let size = icon.size.to_logical::<f64>(scale);
    let (width, height) = PANEL_SIZE;
    let gap = 6.0;
    let mut x = at.x + size.width / 2.0 - width / 2.0;
    let mut y = at.y + size.height + gap;
    if let Some((_, origin, area)) = screen {
        if at.y > origin.y + area.height / 2.0 {
            y = at.y - height - gap; // taskbar at the bottom: open upwards
        }
        x = x
            .min(origin.x + area.width - width - gap)
            .max(origin.x + gap);
    }
    let position = LogicalPosition::new(x, y);
    // macOS positions windows in global points; elsewhere convert at the icon's scale.
    #[cfg(target_os = "macos")]
    let _ = panel.set_position(position);
    #[cfg(not(target_os = "macos"))]
    let _ = panel.set_position(position.to_physical::<f64>(scale));
}

fn allowed(target: &Url) -> bool {
    ours(target)
}

/// Preserve member anchors and explicit preview parameters when leaving the panel.
fn destination(target: &Url) -> String {
    let mut path = target.path().to_owned();
    if let Some(query) = target.query() {
        path.push('?');
        path.push_str(query);
    }
    if let Some(fragment) = target.fragment() {
        path.push('#');
        path.push_str(fragment);
    }
    path
}

/// Bring the main window back on the page it was left on, or open the office if there is none.
fn restore(app: &AppHandle) {
    match app.get_webview_window("main") {
        Some(window) => {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
        None => open(app, "main", "/chat/"),
    }
}

/// Where the main window's last size is kept: this app's own config folder, so the preview build
/// keeps its own and never touches the installed app's.
fn size_file(app: &AppHandle) -> Option<std::path::PathBuf> {
    Some(app.path().app_config_dir().ok()?.join("window-size.json"))
}

fn saved_size(app: &AppHandle) -> Option<(f64, f64)> {
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(size_file(app)?).ok()?).ok()?;
    Some((saved["width"].as_f64()?, saved["height"].as_f64()?))
}

/// Saved when the window closes or the app quits, and only when the size changed.
fn save_size(window: &WebviewWindow) {
    // A maximized or full-screen window says nothing about the size to reopen at.
    let state = |check: tauri::Result<bool>| check.unwrap_or(false);
    if state(window.is_maximized()) || state(window.is_fullscreen()) {
        return;
    }
    let (Some(path), Ok(size), Ok(scale)) = (
        size_file(window.app_handle()),
        window.inner_size(),
        window.scale_factor(),
    ) else {
        return;
    };
    // Windows reports a minimized window as 0×0; other systems keep its real size.
    if size.width == 0 || size.height == 0 {
        return;
    }
    let size = size.to_logical::<f64>(scale);
    let text =
        serde_json::json!({"width": size.width.round(), "height": size.height.round()}).to_string();
    if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

/// Show `path` in the window `label`, creating it on first use.
fn open(app: &AppHandle, label: &str, path: &str) {
    let target = url(path);
    if !allowed(&target) {
        return;
    }
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.navigate(target);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    // The size it was last left at, within the screen; else large enough to show the boss office
    // and a first row of desks, with a margin to the screen's edges.
    let saved = saved_size(app);
    let room = if saved.is_some() { 1.0 } else { 0.95 };
    let (width, height) = saved.unwrap_or(MAIN_SIZE);
    let (max_width, max_height) = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| {
            let area = monitor
                .work_area()
                .size
                .to_logical::<f64>(monitor.scale_factor());
            (area.width * room, area.height * room)
        })
        .unwrap_or(MAIN_SIZE);
    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::CustomProtocol(target))
        .title("AgentNave")
        .inner_size(
            width.min(max_width).max(MAIN_MIN.0),
            height.min(max_height).max(MAIN_MIN.1),
        )
        .min_inner_size(MAIN_MIN.0, MAIN_MIN.1)
        .center();
    // The page's own top bar is the title bar: the window buttons sit in it and follow its theme.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(18.0, 26.0));
    let built = builder
        .on_navigation(allowed)
        .on_new_window(move |target, _| {
            if !ours(&target) && matches!(target.scheme(), "http" | "https") {
                browse(&target);
            }
            NewWindowResponse::Deny
        })
        .build();
    if let Ok(window) = built {
        let closing = window.clone();
        window.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                save_size(&closing);
                // Like other Mac apps, closing only hides the window; the Dock icon brings it back
                // as it was.
                #[cfg(target_os = "macos")]
                {
                    api.prevent_close();
                    let _ = closing.hide();
                }
                #[cfg(not(target_os = "macos"))]
                let _ = api;
            }
        });
        let _ = window.set_focus();
    }
}

/// Links in replies open in the default browser.
fn browse(target: &Url) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(target.as_str()).spawn();
    #[cfg(windows)]
    let _ = Command::new("explorer").arg(target.as_str()).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(target.as_str()).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_build_refuses_every_write_but_moving_its_window() {
        for (method, path) in [
            ("POST", "/app/members"),
            ("POST", "/app/characters/gpt"),
            ("POST", "/app/service/install"),
            ("POST", "/app/update/apply"),
            ("DELETE", "/app/chats/x"),
        ] {
            assert!(preview_refuses(true, method, path), "{method} {path}");
            assert!(!preview_refuses(false, method, path), "{method} {path}");
        }
        assert!(!preview_refuses(true, "GET", "/settings/members"));
        assert!(!preview_refuses(true, "POST", "/app/window/drag"));
    }

    #[test]
    fn windows_stay_on_tray_routes() {
        for path in [
            "/panel/",
            "/manage/",
            "/settings/",
            "/chat/",
            "/chat/group#codex%2Fm",
        ] {
            assert!(allowed(&url(path)), "{path}");
        }
        assert_eq!(
            destination(&url("/chat/group?mock#codex%2Fm")),
            "/chat/group?mock#codex%2Fm"
        );
        assert!(!allowed(&Url::parse("https://example.com/").unwrap()));
    }
}
