//! Window creation, placement, visibility state and native browser handoff.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{
    AppHandle, LogicalPosition, Manager, Rect, Url, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use crate::{PREVIEW, SCHEME, TRAY, navigation, preferences, update, voice};

pub(crate) const PANEL: &str = "panel";
pub(crate) const UPDATE_PAGE: &str = "/chat/?check-update";

const PANEL_SIZE: (f64, f64) = (480.0, 640.0);
/// Matches `--panel-radius` in panel.css, close to the system's menu bar popovers.
#[cfg(target_os = "macos")]
const PANEL_RADIUS: f64 = 16.0;
const MAIN_SIZE: (f64, f64) = (1440.0, 900.0);
const MAIN_MIN: (f64, f64) = (420.0, 480.0);
/// A click on the tray icon first blurs the open panel; this keeps that click from reopening it.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

#[derive(Default)]
pub(crate) struct WindowState {
    panel_hidden: Mutex<Option<Instant>>,
    normal_size: Mutex<Option<(f64, f64)>>,
}

/// What the page's top bar asks of its window. The bar is the title bar on every platform, so it
/// moves, zooms, minimizes and closes the window the way the system's own title bar would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowAction {
    Drag,
    Zoom,
    Minimize,
    Close,
}

impl WindowAction {
    /// The action a request names, or `None` for any other path.
    pub(crate) fn of(path: &str) -> Option<Self> {
        Some(match path {
            "/app/window/drag" => Self::Drag,
            "/app/window/zoom" => Self::Zoom,
            "/app/window/minimize" => Self::Minimize,
            "/app/window/close" => Self::Close,
            _ => return None,
        })
    }

    pub(crate) fn run(self, window: &WebviewWindow) {
        let done = match self {
            Self::Drag => window.start_dragging(),
            Self::Zoom if window.is_maximized().unwrap_or(false) => window.unmaximize(),
            Self::Zoom => window.maximize(),
            Self::Minimize => window.minimize(),
            Self::Close => window.close(),
        };
        let _ = done;
    }
}

pub(crate) fn url(path: &str) -> Url {
    let base = if cfg!(any(windows, target_os = "android")) {
        format!("http://{SCHEME}.localhost")
    } else {
        format!("{SCHEME}://localhost")
    };
    Url::parse(&base)
        .and_then(|base| base.join(path))
        .expect("tray routes are valid paths")
}

pub(crate) fn ours(target: &Url) -> bool {
    let base = url("/");
    target.scheme() == base.scheme() && target.host_str() == base.host_str()
}

pub(crate) fn allowed(target: &Url) -> bool {
    ours(target)
}

/// Preserve member anchors and explicit preview parameters when leaving the panel.
pub(crate) fn destination(target: &Url) -> String {
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

/// The menu bar panel: created hidden at start, so a click shows it at once.
pub(crate) fn build_panel(app: &AppHandle) -> tauri::Result<()> {
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
    .on_page_load(|window, payload| {
        if payload.event() == PageLoadEvent::Finished {
            set_window_hidden(&window, !window.is_visible().unwrap_or(false));
        }
    })
    .on_navigation(move |target| {
        // Panel links name workbench routes or app actions; none navigates the panel itself.
        if !ours(target) || target.path().starts_with("/panel/") {
            return ours(target);
        }
        let (app, path) = (handle.clone(), target.path().to_owned());
        let destination = destination(target);
        thread::spawn(move || match path.as_str() {
            // The panel stays open: its notice shows the progress or "已复制".
            "/app/upgrade-service" if PREVIEW => {}
            "/app/update" => open(&app, TRAY, UPDATE_PAGE),
            "/app/upgrade-service" => update::upgrade_service(&app),
            "/app/copy-service-command" => update::copy_service_command(&app),
            _ => {
                if let Some(panel) = app.get_webview_window(PANEL) {
                    let _ = panel.hide();
                    set_window_hidden(&panel, true);
                }
                if path == "/app/quit" {
                    app.exit(0);
                } else {
                    open(&app, TRAY, &destination);
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
            .build(),
    );
    let panel = builder.build()?;
    #[cfg(target_os = "macos")]
    round_panel(&panel);
    let blurred = app.clone();
    panel.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            hide_panel(&blurred);
        }
    });
    #[cfg(target_os = "macos")]
    hide_panel_on_outside_click(app);
    Ok(())
}

/// Clip the whole window, glass and page together: the glass view's own radius is lost
/// where it reaches past the window's bottom edge.
#[cfg(target_os = "macos")]
fn round_panel(panel: &WebviewWindow) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    let Ok(view) = panel.ns_view() else {
        return;
    };
    // SAFETY: setup runs on the main thread and the window keeps its content view alive.
    unsafe {
        let view = &*view.cast::<AnyObject>();
        let _: () = msg_send![view, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![view, layer];
        if let Some(layer) = layer.as_ref() {
            let _: () = msg_send![layer, setCornerRadius: PANEL_RADIUS];
            let _: () = msg_send![layer, setMasksToBounds: true];
        }
    }
}

/// Hide on losing focus. The time guards the reopen: a click on the menu bar icon first blurs
/// the panel, then toggles it, and that toggle must not show it again.
pub(crate) fn hide_panel(app: &AppHandle) {
    if hide_visible_panel(app) {
        *app.state::<crate::Tray>()
            .windows
            .panel_hidden
            .lock()
            .unwrap() = Some(Instant::now());
    }
}

/// Whether the panel was showing. Once hidden by an outside click it stays hidden on the blur
/// that may follow, so that blur sets no reopen guard either.
pub(crate) fn hide_visible_panel(app: &AppHandle) -> bool {
    let Some(panel) = app.get_webview_window(PANEL) else {
        return false;
    };
    let shown = panel.is_visible().unwrap_or(false);
    if shown {
        let _ = panel.hide();
        set_window_hidden(&panel, true);
    }
    shown
}

/// Recent macOS may refuse to activate the app when its menu bar icon is clicked, so the
/// panel never becomes key and never loses focus. A global monitor sees only clicks sent to
/// other apps: the panel itself and its menu bar icon never trigger it, so it needs no reopen
/// guard and an immediate click on the icon opens the panel again.
#[cfg(target_os = "macos")]
fn hide_panel_on_outside_click(app: &AppHandle) {
    use objc2_app_kit::{NSEvent, NSEventMask};
    let app = app.clone();
    let handler = block2::RcBlock::new(move |_| {
        hide_visible_panel(&app);
    });
    let mask =
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
    // The monitor lives as long as the app.
    std::mem::forget(NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        mask, &handler,
    ));
}

pub(crate) fn toggle_panel(app: &AppHandle, icon: Rect) {
    let Some(panel) = app.get_webview_window(PANEL) else {
        return;
    };
    if panel.is_visible().unwrap_or(false) {
        let _ = panel.hide();
        set_window_hidden(&panel, true);
        return;
    }
    let hidden = *app
        .state::<crate::Tray>()
        .windows
        .panel_hidden
        .lock()
        .unwrap();
    if hidden.is_some_and(|at| at.elapsed() < REOPEN_GUARD) {
        return;
    }
    place(&panel, icon);
    let _ = panel.show();
    set_window_hidden(&panel, false);
    let _ = panel.set_focus();
}

fn clamp_panel_position(
    at: (f64, f64),
    icon_size: (f64, f64),
    screen: Option<((f64, f64), (f64, f64))>,
) -> (f64, f64) {
    let (width, height) = PANEL_SIZE;
    let gap = 6.0;
    let mut x = at.0 + icon_size.0 / 2.0 - width / 2.0;
    let mut y = at.1 + icon_size.1 + gap;
    if let Some(((origin_x, origin_y), (area_width, area_height))) = screen {
        if at.1 > origin_y + area_height / 2.0 {
            y = at.1 - height - gap; // taskbar at the bottom: open upwards
        }
        x = x
            .min(origin_x + area_width - width - gap)
            .max(origin_x + gap);
        y = y
            .min(origin_y + area_height - height - gap)
            .max(origin_y + gap);
    }
    (x, y)
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
    let bounds = screen.map(|(_, origin, area)| ((origin.x, origin.y), (area.width, area.height)));
    let (x, y) = clamp_panel_position((at.x, at.y), (size.width, size.height), bounds);
    let position = LogicalPosition::new(x, y);
    // macOS positions windows in global points; elsewhere convert at the icon's scale.
    #[cfg(target_os = "macos")]
    let _ = panel.set_position(position);
    #[cfg(not(target_os = "macos"))]
    let _ = panel.set_position(position.to_physical::<f64>(scale));
}

pub(crate) fn set_window_hidden(window: &WebviewWindow, hidden: bool) {
    let script = if hidden {
        "document.documentElement.dataset.windowHidden = 'true'; window.dispatchEvent(new Event('agentnave:visibility'));"
    } else {
        "delete document.documentElement.dataset.windowHidden; window.dispatchEvent(new Event('agentnave:visibility'));"
    };
    let _ = window.eval(script);
}

/// Tell the page its window is behind others (or back in front), for pages that quiet down then.
fn set_window_blurred(window: &WebviewWindow, blurred: bool) {
    let script = if blurred {
        "document.documentElement.dataset.windowBlurred = 'true'; window.dispatchEvent(new Event('agentnave:visibility'));"
    } else {
        "delete document.documentElement.dataset.windowBlurred; window.dispatchEvent(new Event('agentnave:visibility'));"
    };
    let _ = window.eval(script);
}

/// Bring the main window back on the page it was left on, or open the office if there is none.
pub(crate) fn restore(app: &AppHandle) {
    match app.get_webview_window(TRAY) {
        Some(window) => {
            let _ = window.unminimize();
            let _ = window.show();
            set_window_hidden(&window, false);
            let _ = window.set_focus();
        }
        None => open(app, TRAY, "/chat/"),
    }
}

/// Where the main window's last size is kept: this app's own config folder, so the preview build
/// keeps its own and never touches the installed app's.
fn size_file(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_config_dir().ok()?.join("window-size.json"))
}

fn saved_size(app: &AppHandle) -> Option<(f64, f64)> {
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(size_file(app)?).ok()?).ok()?;
    Some((saved["width"].as_f64()?, saved["height"].as_f64()?))
}

fn remember_normal_size(window: &WebviewWindow) {
    let state = |check: tauri::Result<bool>| check.unwrap_or(false);
    if state(window.is_maximized()) || state(window.is_fullscreen()) {
        return;
    }
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    // Windows reports a minimized window as 0×0; other systems keep its real size.
    if size.width == 0 || size.height == 0 {
        return;
    }
    let logical = size.to_logical::<f64>(scale);
    if let Some(tray) = window.app_handle().try_state::<crate::Tray>() {
        *tray.windows.normal_size.lock().unwrap() =
            Some((logical.width.round(), logical.height.round()));
    }
}

/// Saved when the window closes or the app quits, keeping the last normal (unmaximized) size.
pub(crate) fn save_size(window: &WebviewWindow) {
    remember_normal_size(window);
    let Some(path) = size_file(window.app_handle()) else {
        return;
    };
    let Some((width, height)) = window
        .app_handle()
        .try_state::<crate::Tray>()
        .and_then(|tray| *tray.windows.normal_size.lock().unwrap())
    else {
        return;
    };
    let text = serde_json::json!({"width": width.round(), "height": height.round()}).to_string();
    if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

/// Show `path` in the window `label`, creating it on first use.
pub(crate) fn open(app: &AppHandle, label: &str, path: &str) {
    let target = url(path);
    if !allowed(&target) {
        return;
    }
    if let Some(window) = app.get_webview_window(label) {
        if !navigation::in_place(&window, &target) {
            let _ = window.navigate(target);
        }
        let _ = window.unminimize();
        let _ = window.show();
        set_window_hidden(&window, false);
        let _ = window.set_focus();
        return;
    }
    build_main(app, label, target, true);
}

/// Opens the office for listening without bringing it forward: a hidden window on another page
/// goes back to the office, and without a window one is created hidden. Standalone settings and
/// management pages stay, even hidden, to preserve unsaved edits. Returns whether it is opening.
pub(crate) fn open_office_hidden(app: &AppHandle) -> bool {
    let target = url("/chat/");
    let Some(window) = app.get_webview_window(TRAY) else {
        build_main(app, TRAY, target, false);
        return true;
    };
    if let Ok(current) = window.url() {
        if current.path().starts_with("/chat/") {
            return true;
        }
        if current.path().starts_with("/settings/") || current.path().starts_with("/manage/") {
            return false;
        }
    }
    if window.is_visible().unwrap_or(false) {
        return false;
    }
    let _ = window.navigate(target);
    true
}

fn build_main(app: &AppHandle, label: &str, target: Url, show: bool) {
    // The size it was last left at, within the screen; else large enough to show the boss office
    // and a first row of desks, with a margin to the screen's edges.
    let saved = saved_size(app);
    if let (Some(size), Some(tray)) = (saved, app.try_state::<crate::Tray>()) {
        *tray.windows.normal_size.lock().unwrap() = Some(size);
    }
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
        .visible(show)
        .center();
    // The page's own top bar is the title bar: the window buttons sit in it and follow its theme.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(18.0, 26.0));
    // Windows has no overlay title bar: drop its frame and let the same bar carry Windows' own
    // caption buttons at its right end.
    #[cfg(windows)]
    let builder = builder.decorations(false);
    let built = builder
        .on_navigation(allowed)
        // A main-document navigation clears the player; a settings iframe keeps that document.
        .on_page_load(|window, payload| {
            if window.label() == TRAY {
                match payload.event() {
                    PageLoadEvent::Started => voice::player::clear(window.app_handle()),
                    PageLoadEvent::Finished => {
                        set_window_hidden(&window, !window.is_visible().unwrap_or(true));
                    }
                }
            }
        })
        .on_new_window(move |target, _| {
            if !ours(&target) && matches!(target.scheme(), "http" | "https") {
                browse(&target);
            }
            NewWindowResponse::Deny
        })
        .build();
    if let Ok(window) = built {
        remember_normal_size(&window);
        if let Err(error) = preferences::runtime::zoom(&window) {
            eprintln!("无法设置窗口缩放：{error}");
        }
        let tracked = window.clone();
        window.on_window_event(move |event| match event {
            WindowEvent::Resized(_) => remember_normal_size(&tracked),
            WindowEvent::Destroyed => voice::player::clear(tracked.app_handle()),
            WindowEvent::Focused(focused) => set_window_blurred(&tracked, !focused),
            WindowEvent::CloseRequested { api, .. } => {
                save_size(&tracked);
                // Like other Mac apps, closing only hides the window; the Dock icon brings it back
                // as it was.
                #[cfg(target_os = "macos")]
                {
                    api.prevent_close();
                    let _ = tracked.hide();
                    set_window_hidden(&tracked, true);
                }
                #[cfg(not(target_os = "macos"))]
                let _ = api;
            }
            _ => {}
        });
        if show {
            let _ = window.set_focus();
        }
    }
}

/// Opens a sign-in page in the default browser.
pub(crate) fn browse_address(address: &str) {
    if let Ok(target) = address.parse::<Url>() {
        browse(&target);
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
    fn the_top_bar_asks_for_window_actions_by_path() {
        assert_eq!(
            WindowAction::of("/app/window/drag"),
            Some(WindowAction::Drag)
        );
        assert_eq!(
            WindowAction::of("/app/window/zoom"),
            Some(WindowAction::Zoom)
        );
        assert_eq!(
            WindowAction::of("/app/window/minimize"),
            Some(WindowAction::Minimize)
        );
        assert_eq!(
            WindowAction::of("/app/window/close"),
            Some(WindowAction::Close)
        );
        for path in [
            "/app/window/state",
            "/app/window/",
            "/app/status",
            "/window/drag",
        ] {
            assert_eq!(WindowAction::of(path), None, "{path}");
        }
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

    #[test]
    fn panel_placement_clamps_both_axes_inside_short_and_offset_screens() {
        // Bottom taskbar on a short scaled display (576 logical px high): top edge stays >= origin.y + gap.
        let (x, y) = clamp_panel_position(
            (20.0, 540.0),
            (24.0, 24.0),
            Some(((0.0, 0.0), (1024.0, 576.0))),
        );
        assert_eq!(x, 6.0);
        assert_eq!(y, 6.0);

        // Top menu bar on a secondary display with negative Y origin.
        let (x, y) = clamp_panel_position(
            (1800.0, -1070.0),
            (24.0, 22.0),
            Some(((0.0, -1080.0), (1920.0, 1080.0))),
        );
        assert_eq!(x, 1920.0 - PANEL_SIZE.0 - 6.0);
        assert_eq!(y, -1042.0);
    }
}
