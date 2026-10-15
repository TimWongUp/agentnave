//! AgentNave tray: a read-only menu bar view of the Studio conversations on this machine.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod data;

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
    AppHandle, Manager, PhysicalPosition, Rect, RunEvent, Url, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use data::{CSP, Overview, Reader};

const SCHEME: &str = "agentnave";
const TRAY: &str = "main";
const PANEL: &str = "panel";
const PANEL_SIZE: (f64, f64) = (360.0, 520.0);
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
        .register_asynchronous_uri_scheme_protocol(SCHEME, move |_, request, responder| {
            let reader = served.clone();
            let path = request.uri().path().to_owned();
            thread::spawn(move || {
                let reply = reader.respond(&path);
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
            let menu = Menu::with_items(
                handle,
                &[
                    &MenuItem::with_id(handle, "open", "打开工作台", true, None::<&str>)?,
                    &MenuItem::with_id(handle, "quit", "退出 AgentNave", true, None::<&str>)?,
                ],
            )?;
            TrayIconBuilder::with_id(TRAY)
                .icon(Image::from_bytes(include_bytes!("../icons/tray@2x.png"))?)
                .icon_as_template(true)
                .tooltip(headline(&overview))
                .title(title(&overview).unwrap_or_default())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => open(app, "main", "/"),
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
            thread::spawn(move || poll(handle, reader, overview));
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
        // Panel links name workbench routes: open them in the main window instead.
        if !ours(target) || target.path().starts_with("/panel/") {
            return ours(target);
        }
        let (app, path) = (handle.clone(), target.path().to_owned());
        thread::spawn(move || {
            if let Some(panel) = app.get_webview_window(PANEL) {
                let _ = panel.hide();
            }
            if path == "/app/quit" {
                app.exit(0);
            } else {
                open(&app, "main", &path);
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
    let monitor = panel
        .available_monitors()
        .unwrap_or_default()
        .into_iter()
        .find(|monitor| {
            let (origin, size) = (monitor.position(), monitor.size());
            let point = icon.position.to_physical::<f64>(monitor.scale_factor());
            point.x >= origin.x as f64
                && point.x < origin.x as f64 + size.width as f64
                && point.y >= origin.y as f64
                && point.y < origin.y as f64 + size.height as f64
        });
    let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor());
    let at = icon.position.to_physical::<f64>(scale);
    let size = icon.size.to_physical::<f64>(scale);
    let (width, height) = (PANEL_SIZE.0 * scale, PANEL_SIZE.1 * scale);
    let gap = 6.0 * scale;
    let mut x = at.x + size.width / 2.0 - width / 2.0;
    let mut y = at.y + size.height + gap;
    if let Some(monitor) = monitor {
        let (origin, area) = (monitor.position(), monitor.size());
        let (left, top) = (origin.x as f64, origin.y as f64);
        if at.y > top + area.height as f64 / 2.0 {
            y = at.y - height - gap;
        }
        x = x.clamp(left + gap, left + area.width as f64 - width - gap);
    }
    let _ = panel.set_position(PhysicalPosition::new(x, y));
}

/// Show `path` in the window `label`, creating it on first use.
fn open(app: &AppHandle, label: &str, path: &str) {
    let target = url(path);
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
        .on_navigation(ours)
        .on_new_window(move |target, _| {
            if ours(&target) {
                // "选手视角 ↗" opens the public view beside the workbench.
                let app = handle.clone();
                thread::spawn(move || open(&app, "public", target.path()));
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
