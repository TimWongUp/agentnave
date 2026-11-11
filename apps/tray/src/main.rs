//! AgentNave tray: view and clean up this machine's saved Studio conversations.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alerts;
mod avatars;
mod badge;
mod characters;
mod config_paths;
mod data;
mod data_files;
mod db;
mod detect;
mod hosts;
mod manage;
mod members;
mod navigation;
mod orchestration_roles;
mod power;
mod preferences;
mod process;
mod provider_health;
mod providers;
mod proxy;
mod response;
mod rules;
mod settings;
mod stage;
mod storage;
mod time_format;
mod tool_dirs;
mod update;
mod video_packs;
mod voice;
mod window;

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tauri::http::{Response, header};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent};

use data::snapshot::Overview;
use data::{CSP, Reader};
use update::Updates;
use window::{UPDATE_PAGE, WindowAction, WindowState};

const SCHEME: &str = "agentnave";
const TRAY: &str = "main";
const POLL: Duration = Duration::from_secs(2);
/// A preview build (`scripts/preview_tray.sh`) runs beside the installed app on the same data to
/// show UI changes: it never changes production data, host configs or Skills, and never updates
/// or installs. Its display preferences and voice settings, keys and cache stay separate.
pub const PREVIEW: bool = option_env!("AGENTNAVE_PREVIEW").is_some();

/// Preview writes only its own appearance and voice data. Other exceptions move the window,
/// re-probe CLIs in memory, check the release feed or open a rules file in the user's editor.
/// Header values are ASCII; the page reads this back with decodeURIComponent.
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn preview_refuses(preview: bool, method: &str, path: &str) -> bool {
    preview
        && method != "GET"
        && !(method == "POST"
            && ((cfg!(unix) && path == "/app/hosts/reload-copy")
                || path.starts_with("/app/video-packs/")
                || matches!(
                    path,
                    "/app/preferences"
                        | "/app/alerts/test"
                        | "/settings/members"
                        | "/app/voice"
                        | "/app/voice/key"
                        | "/app/voice/speak"
                        | "/app/voice/cache"
                        | "/app/voice/cache/clear"
                        | "/app/voice/summary/models"
                        | "/app/voice/chatgpt/start"
                        | "/app/voice/chatgpt/sign-out"
                        | "/app/voice/player"
                        | "/app/voice/player/command"
                        | "/app/window/drag"
                        | "/app/window/zoom"
                        | "/app/window/minimize"
                        | "/app/window/close"
                        | "/app/update/check"
                        | "/app/rules/open"
                        | "/app/storage/open"
                        | "/app/theme/system"
                        | "/app/theme/light"
                        | "/app/theme/dark"
                )))
}

pub(crate) struct Tray {
    pub(crate) reader: Arc<Reader>,
    pub(crate) windows: WindowState,
}

fn main() {
    proxy::apply();
    let reader = Arc::new(Reader::new(data_files::data_root()));
    #[cfg(target_os = "macos")]
    video_packs::http::start(data_files::data_root());
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            window::restore(app)
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(preferences::runtime::Runtime::default());
    let app = register_protocol(builder, reader.clone())
        .manage(Tray {
            reader: reader.clone(),
            windows: WindowState::default(),
        })
        .setup(move |app| setup(app, reader))
        .build(tauri::generate_context!())
        .expect("failed to start the AgentNave tray");
    app.run(|app, event| match event {
        // Closing the last window keeps the tray running; only "退出" ends the app.
        RunEvent::ExitRequested {
            api, code: None, ..
        } => api.prevent_exit(),
        // Like any regular app, launching it shows the main window. Opening it once the event
        // loop runs keeps its first requests from stalling behind start-up.
        RunEvent::Ready => {
            if preferences::general::current()["open_on_start"] == true {
                window::open(app, TRAY, "/chat/");
            }
        }
        RunEvent::Exit => {
            preferences::runtime::shutdown(app);
            if let Some(window) = app.get_webview_window(TRAY) {
                window::save_size(&window);
            }
        }
        // A click on the Dock icon brings the main window back where it was left.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => window::restore(app),
        _ => {}
    });
}

fn poll(app: AppHandle, reader: Arc<Reader>, mut shown: Overview) {
    loop {
        thread::sleep(POLL);
        let overview = reader.overview();
        if let Err(error) = preferences::runtime::refresh_working(&app, &overview) {
            eprintln!("无法应用常规设置：{error}");
        }
        if overview == shown {
            continue;
        }
        if let Some(tray) = app.tray_by_id(TRAY) {
            let _ = tray.set_tooltip(Some(headline(&overview)));
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

pub(crate) fn browse_address(address: &str) {
    window::browse_address(address);
}

fn register_protocol(
    builder: tauri::Builder<tauri::Wry>,
    served: Arc<Reader>,
) -> tauri::Builder<tauri::Wry> {
    builder.register_asynchronous_uri_scheme_protocol(SCHEME, move |ctx, request, responder| {
        let reader = served.clone();
        let app = ctx.app_handle().clone();
        let path = match request.uri().path() {
            "/app/storage/preview" => request
                .uri()
                .path_and_query()
                .map(|path| path.as_str())
                .unwrap_or("/app/storage/preview")
                .to_owned(),
            path => path.to_owned(),
        };
        let label = ctx.webview_label().to_owned();
        let flag = |name: &str| {
            request
                .uri()
                .query()
                .is_some_and(|query| query.split('&').any(|pair| pair == name))
        };
        let quiet = flag("quiet=1");
        let service_only = flag("service=1");
        thread::spawn(move || {
            let reply = ProtocolRequest {
                app,
                reader,
                label,
                path,
                quiet,
                service_only,
                request,
            }
            .reply();
            let mut response = Response::builder();
            if let Some(notice) = &reply.notice {
                response = response.header("X-AgentNave-Notice", percent_encode(notice));
            }
            for (name, value) in &reply.headers {
                response = response.header(*name, value);
            }
            let response = response
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
}

fn setup(app: &mut tauri::App, reader: Arc<Reader>) -> Result<(), Box<dyn std::error::Error>> {
    // A regular app: it has a Dock icon and can be kept in the Dock; the menu bar icon stays.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Regular);
    let handle = app.handle();
    power::watch(handle.clone());
    if let Err(error) = preferences::autostart::repair(handle) {
        eprintln!("无法修复开机启动路径：{error}");
    }
    let host_context = hosts::Context::live(handle.path().app_data_dir()?);
    let takeover = reader.clone();
    window::build_panel(handle)?;
    let overview = reader.overview();
    let menu = preferences::runtime::menu(handle)?;
    if let Err(error) = preferences::runtime::apply(handle) {
        eprintln!("无法应用常规设置：{error}");
    }
    TrayIconBuilder::with_id(TRAY)
        .icon(badge::initial(
            badge::count(&overview, preferences::general::current),
            &preferences::general::current(),
        ))
        .icon_as_template(true)
        .tooltip(headline(&overview))
        .title(if PREVIEW { "预览" } else { "" })
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => window::restore(app),
            "settings" => window::open(app, TRAY, "/settings/"),
            "update" if !PREVIEW => window::open(app, TRAY, UPDATE_PAGE),
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
                badge::drip(tray, &preferences::general::current());
                if preferences::general::current()["tray_click"] == "office" {
                    window::hide_visible_panel(tray.app_handle());
                    window::open(tray.app_handle(), TRAY, "/chat/");
                } else {
                    window::toggle_panel(tray.app_handle(), rect);
                }
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
            // After the repair, which settles each boss's status; probing the CLIs takes a while.
            takeover.prepare_members();
        });
        if !PREVIEW {
            update::resume(&resumer);
            thread::spawn(move || {
                loop {
                    update::auto_check(&checker);
                    thread::sleep(update::EVERY);
                }
            });
        }
    });
    alerts::start(handle.clone(), reader.clone());
    thread::spawn(move || poll(handle, reader, overview));
    Ok(())
}

struct ProtocolRequest {
    app: AppHandle,
    reader: Arc<Reader>,
    label: String,
    path: String,
    quiet: bool,
    service_only: bool,
    request: tauri::http::Request<Vec<u8>>,
}

impl ProtocolRequest {
    fn reply(&self) -> response::Response {
        let Self {
            app,
            reader,
            label,
            path,
            quiet,
            service_only,
            ..
        } = self;
        let (quiet, service_only) = (*quiet, *service_only);
        let method = self.request.method().as_str();
        let action = self
            .request
            .headers()
            .get("X-AgentNave-Action")
            .and_then(|v| v.to_str().ok());
        match (method, path.as_str()) {
            _ if preview_refuses(PREVIEW, method, path) => {
                response::Response::error(403, "预览版只读：请在正式版里保存改动")
            }
            ("GET", path) if path.starts_with("/art/video/") => reader.video_asset(
                path,
                self.request
                    .headers()
                    .get(header::RANGE)
                    .and_then(|v| v.to_str().ok()),
            ),
            _ if path == "/app/video-packs" || path.starts_with("/app/video-packs/") => {
                reader.video_packs_request(Some(app), method, path, action, self.request.body())
            }
            ("GET", "/app/status") => response::Response::json(&update::status(app)),
            (_, "/app/alerts/test") => alerts::test(app, method, action),
            // Whether the page's maximize button should read as maximize or restore.
            ("GET", "/app/window/state") => {
                let maximized = app
                    .get_webview_window(label)
                    .and_then(|window| window.is_maximized().ok())
                    .unwrap_or(false);
                response::Response::json(&serde_json::json!({ "maximized": maximized }))
            }
            // The top bar moves, zooms, minimizes and closes its window, as a native title
            // bar would; the main window draws no title bar of its own.
            (
                _,
                "/app/window/drag"
                | "/app/window/zoom"
                | "/app/window/minimize"
                | "/app/window/close",
            ) => response::action(method, action, || {
                match (WindowAction::of(path), app.get_webview_window(label)) {
                    (Some(step), Some(window)) => {
                        step.run(&window);
                        response::Response::json(&serde_json::json!({"ok": true}))
                    }
                    _ => response::Response::error(404, "找不到窗口"),
                }
            }),
            // Native appearance (the panel's glass, window buttons) follows the page's theme.
            (_, "/app/theme/system" | "/app/theme/light" | "/app/theme/dark") => {
                response::action(method, action, || {
                    let choice = path.rsplit('/').next().unwrap_or("system");
                    match preferences::runtime::save(
                        app,
                        &serde_json::json!({"general": {"theme": choice}}),
                    ) {
                        Ok(_) => response::Response::json(&serde_json::json!({"ok": true})),
                        Err(error) => response::Response::error(400, &error),
                    }
                })
            }
            (_, "/app/voice/player" | "/app/voice/player/command") => {
                voice::player::request(app, label, method, path, action, self.request.body())
            }
            (_, "/app/preferences") => {
                preferences::runtime::request(app, method, action, self.request.body())
            }
            ("GET", "/settings/members") => reader.members(false),
            (_, "/settings/members") => response::action(method, action, || reader.members(true)),
            (_, "/app/service/install") => response::action(method, action, || {
                response::Response::json(&update::install_service_now(app))
            }),
            (_, "/app/update/check") => response::action(method, action, || {
                response::Response::json(&update::check_now(app, !quiet))
            }),
            (_, "/app/update/apply") => response::action(method, action, || {
                response::Response::json(&update::apply(app, service_only))
            }),
            _ => reader.request(method, path, action, self.request.body()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_build_refuses_every_write_but_moving_its_window() {
        for (method, path) in [
            ("POST", "/app/members"),
            ("POST", "/app/provider-health/clear"),
            ("POST", "/app/settings"),
            ("POST", "/app/characters/gpt"),
            ("POST", "/app/service/install"),
            ("POST", "/app/update/apply"),
            ("POST", "/app/cli-updates/update"),
            ("DELETE", "/app/chats/x"),
            ("DELETE", "/settings/members"),
        ] {
            assert!(preview_refuses(true, method, path), "{method} {path}");
            assert!(!preview_refuses(false, method, path), "{method} {path}");
        }
        for path in [
            "/app/preferences",
            "/app/alerts/test",
            "/settings/members",
            "/app/voice",
            "/app/voice/key",
            "/app/voice/speak",
            "/app/voice/cache",
            "/app/voice/cache/clear",
            "/app/voice/summary/models",
            "/app/voice/chatgpt/start",
            "/app/voice/chatgpt/sign-out",
        ] {
            assert!(!preview_refuses(true, "POST", path));
        }
        assert!(preview_refuses(true, "POST", "/app/voice/unknown"));
        assert!(!preview_refuses(true, "GET", "/settings/members"));
        assert!(!preview_refuses(true, "POST", "/app/window/drag"));
        assert!(!preview_refuses(true, "POST", "/app/window/minimize"));
        assert!(!preview_refuses(true, "POST", "/app/window/close"));
        assert!(!preview_refuses(true, "POST", "/app/update/check"));
        assert!(!preview_refuses(true, "POST", "/app/rules/open"));
        assert!(!preview_refuses(true, "POST", "/app/storage/open"));
        assert!(!preview_refuses(
            true,
            "GET",
            "/app/storage/preview?days=30"
        ));
        assert!(!preview_refuses(true, "POST", "/app/theme/dark"));
        assert_eq!(
            preview_refuses(true, "POST", "/app/hosts/reload-copy"),
            !cfg!(unix)
        );
    }
}
