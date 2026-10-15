//! AgentNave tray: a read-only menu bar view of the Studio conversations on this machine.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod data;

use std::fs;
use std::path::PathBuf;
#[cfg(any(target_os = "macos", windows))]
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tauri::http::{Response, header};
use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, Wry};

use data::{CSP, Overview, Reader};

const SCHEME: &str = "agentnave";
const TRAY: &str = "main";
const POLL: Duration = Duration::from_secs(2);
const MENU_ITEMS: usize = 10;

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
                let cache = if reply.cacheable {
                    "private, max-age=86400"
                } else {
                    "no-store"
                };
                let response = Response::builder()
                    .status(reply.status)
                    .header(header::CONTENT_TYPE, reply.content_type)
                    .header(header::CACHE_CONTROL, cache)
                    .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                    .header(header::REFERRER_POLICY, "no-referrer")
                    .header(header::CONTENT_SECURITY_POLICY, CSP)
                    .body(reply.body)
                    .expect("static response headers are valid");
                responder.respond(response);
            });
        })
        .manage(reader.clone())
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let overview = reader.overview();
            TrayIconBuilder::with_id(TRAY)
                .icon(Image::from_bytes(include_bytes!("../icons/tray@2x.png"))?)
                .icon_as_template(true)
                .tooltip(headline(&overview))
                .title(title(&overview).unwrap_or_default())
                .menu(&menu(app.handle(), &reader, &overview)?)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    id => {
                        if let Some(path) = id.strip_prefix("open:") {
                            open(app, "main", path);
                        }
                    }
                })
                .build(app)?;
            let handle = app.handle().clone();
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
            continue; // rebuilding an open menu would close it
        }
        if let (Some(tray), Ok(menu)) = (app.tray_by_id(TRAY), menu(&app, &reader, &overview)) {
            let _ = tray.set_menu(Some(menu));
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
    format!(
        "AgentNave · 运行中 {} · 待审核 {}",
        overview.count("running"),
        overview.count("review")
    )
}

fn menu(app: &AppHandle, reader: &Reader, overview: &Overview) -> tauri::Result<Menu<Wry>> {
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
    let note = if overview.missing {
        format!("未找到数据目录 {}", reader.root().display())
    } else {
        headline(overview)
    };
    items.push(Box::new(MenuItem::with_id(
        app,
        "headline",
        note,
        false,
        None::<&str>,
    )?));
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    let active: Vec<_> = overview
        .items
        .iter()
        .filter(|item| !item.archived)
        .take(MENU_ITEMS)
        .collect();
    if active.is_empty() {
        items.push(Box::new(MenuItem::with_id(
            app,
            "empty",
            "暂无未结束的会话",
            false,
            None::<&str>,
        )?));
    }
    for item in active {
        let mode = if item.mode == "task" {
            "任务"
        } else {
            "节目"
        };
        let state = match item.status.as_str() {
            "running" => " · 进行中",
            "review" => " · 待审核",
            "failed" | "blocked" | "interrupted" => " · 需处理",
            _ => "",
        };
        let mut title: String = item.title.chars().take(36).collect();
        if title.len() < item.title.len() {
            title.push('…');
        }
        let label = format!("{mode}｜{title}{state}");
        items.push(Box::new(MenuItem::with_id(
            app,
            format!("open:/c/{}/", item.id),
            label,
            true,
            None::<&str>,
        )?));
    }
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "open:/",
        "打开工作台…",
        true,
        None::<&str>,
    )?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "quit",
        "退出 AgentNave",
        true,
        None::<&str>,
    )?));
    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|item| item.as_ref()).collect();
    Menu::with_items(app, &refs)
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
    let reader = app.state::<Arc<Reader>>().inner().clone();
    let built = WebviewWindowBuilder::new(app, label, WebviewUrl::CustomProtocol(target))
        .title("AgentNave")
        .inner_size(1120.0, 820.0)
        .min_inner_size(420.0, 480.0)
        .on_navigation(ours)
        .on_new_window(move |target, _| {
            // "选手看到的页面 ↗" opens the public view beside the director view.
            if ours(&target) {
                let app = handle.clone();
                thread::spawn(move || open(&app, "public", target.path()));
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
