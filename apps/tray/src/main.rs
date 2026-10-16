//! AgentNave tray: view and clean up this machine's saved Studio conversations.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod data;
mod manage;
mod update;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tauri::http::{Response, header};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{
    AppHandle, LogicalPosition, Manager, Rect, RunEvent, Url, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use data::{CSP, Overview, Reader};
use update::Updates;

const SCHEME: &str = "agentnave";
const TRAY: &str = "main";
const PANEL: &str = "panel";
const PUBLIC: &str = "public";
const PANEL_SIZE: (f64, f64) = (380.0, 600.0);
const POLL: Duration = Duration::from_secs(2);
/// A click on the tray icon first blurs the open panel; this keeps that click from reopening it.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

struct Tray {
    reader: Arc<Reader>,
    panel_hidden: Mutex<Option<Instant>>,
}

fn main() {
    let reader = Arc::new(Reader::new(data::data_root()));
    let served = reader.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            open(app, "main", "/")
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .register_asynchronous_uri_scheme_protocol(SCHEME, move |ctx, request, responder| {
            let reader = served.clone();
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_owned();
            thread::spawn(move || {
                let reply = if request.method() == "GET" && path == "/app/status" {
                    data::Response::json(&update::status(&app))
                } else {
                    reader.request(
                        request.method().as_str(),
                        &path,
                        request
                            .headers()
                            .get("X-AgentNave-Action")
                            .and_then(|v| v.to_str().ok()),
                        request.body(),
                    )
                };
                let response = Response::builder()
                    .status(reply.status)
                    .header(header::CONTENT_TYPE, reply.content_type)
                    .header(header::CACHE_CONTROL, "no-store")
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
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle();
            build_panel(handle)?;
            let overview = reader.overview();
            // Left click opens the panel; right click shows this short menu.
            let check = MenuItem::with_id(handle, "update", "检查更新…", true, None::<&str>)?;
            let menu = Menu::with_items(
                handle,
                &[
                    &MenuItem::with_id(handle, "open", "打开工作台", true, None::<&str>)?,
                    &MenuItem::with_id(handle, "manage", "管理会话…", true, None::<&str>)?,
                    &check,
                    &MenuItem::with_id(handle, "quit", "退出 AgentNave", true, None::<&str>)?,
                ],
            )?;
            app.manage(Updates::new(check));
            TrayIconBuilder::with_id(TRAY)
                .icon(Image::from_bytes(include_bytes!("../icons/tray@2x.png"))?)
                .icon_as_template(true)
                .tooltip(headline(&overview))
                .title(title(&overview).unwrap_or_default())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => open(app, "main", "/"),
                    "manage" => open(app, "main", "/manage/"),
                    "update" => update::act(app),
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
                        toggle_panel(tray.app_handle(), rect);
                    }
                })
                .build(app)?;
            let handle = handle.clone();
            let checker = handle.clone();
            thread::spawn(move || poll(handle, reader, overview));
            thread::spawn(move || {
                loop {
                    update::check(&checker, false);
                    thread::sleep(update::EVERY);
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start the AgentNave tray");
    app.run(|_, event| {
        // Closing the last window keeps the tray running; only "退出" ends the app.
        if let RunEvent::ExitRequested {
            api, code: None, ..
        } = event
        {
            api.prevent_exit();
        }
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
            let _ = tray.set_title(title(&overview));
            let _ = tray.set_tooltip(Some(headline(&overview)));
        }
        shown = overview;
    }
}

/// Text beside the menu bar icon, only while something needs attention.
fn title(overview: &Overview) -> Option<String> {
    let parts: Vec<String> = [
        (overview.count("running"), "运行"),
        (overview.count("review"), "待审"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn headline(overview: &Overview) -> String {
    if overview.missing {
        return "AgentNave · 尚未找到工作室数据".to_owned();
    }
    format!(
        "AgentNave · 运行中 {} · 待审核 {}",
        overview.count("running"),
        overview.count("review")
    )
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
        thread::spawn(move || match path.as_str() {
            // The panel stays open: its notice shows the progress or "已复制".
            "/app/update" => update::act(&app),
            "/app/upgrade-service" => update::upgrade_service(&app),
            "/app/copy-service-command" => update::copy_service_command(),
            _ => {
                if let Some(panel) = app.get_webview_window(PANEL) {
                    let _ = panel.hide();
                }
                if path == "/app/quit" {
                    app.exit(0);
                } else {
                    open(&app, "main", &path);
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

/// The public window only ever shows public views, whatever a link in a reply points at.
fn allowed(label: &str, target: &Url) -> bool {
    ours(target) && (label != PUBLIC || target.path().starts_with("/p/"))
}

/// Show `path` in the window `label`, creating it on first use.
fn open(app: &AppHandle, label: &str, path: &str) {
    let target = url(path);
    if !allowed(label, &target) {
        return;
    }
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.navigate(target);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let handle = app.clone();
    let reader = app.state::<Tray>().reader.clone();
    let built = WebviewWindowBuilder::new(app, label, WebviewUrl::CustomProtocol(target))
        .title("AgentNave")
        .inner_size(1180.0, 800.0)
        .min_inner_size(420.0, 480.0)
        .on_navigation({
            let label = label.to_owned();
            move |target| allowed(&label, target)
        })
        .on_new_window(move |target, _| {
            if ours(&target) {
                // "选手视角 ↗" opens the public view beside the workbench; nothing else may.
                if target.path().starts_with("/p/") {
                    let app = handle.clone();
                    thread::spawn(move || open(&app, PUBLIC, target.path()));
                }
            } else if matches!(target.scheme(), "http" | "https") {
                browse(&target);
            }
            NewWindowResponse::Deny
        })
        .on_download(move |_, event| {
            match event {
                DownloadEvent::Requested { url, destination } => *destination = download_path(&url),
                // WebKit cannot download from a custom scheme, so save the export ourselves.
                DownloadEvent::Finished {
                    url,
                    success: false,
                    ..
                } if ours(&url) => {
                    let reader = reader.clone();
                    thread::spawn(move || save(&reader, &url));
                }
                _ => {}
            }
            true
        })
        .build();
    if let Ok(window) = built {
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

/// Write an export into Downloads and show it in the file manager.
fn save(reader: &Reader, url: &Url) {
    let reply = reader.respond(url.path());
    let path = download_path(url);
    if reply.status != 200 || fs::write(&path, reply.body).is_err() {
        return;
    }
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg("-R").arg(&path).spawn();
    #[cfg(windows)]
    let _ = Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

/// `~/Downloads/agentnave-<id>.jsonl`, numbered instead of overwriting.
fn download_path(url: &Url) -> PathBuf {
    let folder = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default();
    let id = url
        .path()
        .split('/')
        .filter(|part| !part.is_empty())
        .nth(1)
        .unwrap_or("transcript");
    let mut path = folder.join(format!("agentnave-{id}.jsonl"));
    for n in 1.. {
        if !path.exists() {
            break;
        }
        path = folder.join(format!("agentnave-{id}-{n}.jsonl"));
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_window_stays_on_public_views() {
        assert!(allowed(PUBLIC, &url("/p/a1/")));
        for path in ["/", "/c/a1/", "/panel/", "/manage/", "/app/quit"] {
            assert!(!allowed(PUBLIC, &url(path)), "{path}");
            assert!(allowed("main", &url(path)), "{path}");
        }
        assert!(!allowed(
            "main",
            &Url::parse("https://example.com/").unwrap()
        ));
    }
}
