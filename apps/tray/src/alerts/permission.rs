//! Query native permissions: the plugin reports Granted unconditionally on desktop.

fn text<'a>(english: bool, zh: &'a str, en: &'a str) -> &'a str {
    if english { en } else { zh }
}

fn error(english: bool, detail: impl std::fmt::Display) -> String {
    format!(
        "{}：{detail}",
        text(
            english,
            "无法发送系统通知",
            "Unable to send a system notification"
        )
    )
}

#[cfg(any(windows, test))]
fn identity(identifier: &str, preview: bool) -> &str {
    if preview {
        "io.github.timwongup.agentnave.preview"
    } else {
        identifier
    }
}

#[cfg(target_os = "macos")]
pub(super) fn check(_app: &tauri::AppHandle, request: bool, english: bool) -> Result<(), String> {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSBundle;
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSettings,
        UNUserNotificationCenter,
    };
    use std::ptr::NonNull;
    use std::sync::{OnceLock, mpsc};
    use std::time::Duration;

    // Outside a bundle UNUserNotificationCenter raises an Objective-C exception.
    let identifier = NSBundle::mainBundle()
        .bundleIdentifier()
        .ok_or_else(|| {
            text(
                english,
                "系统通知需要从 App 应用包运行",
                "Notifications require the bundled App",
            )
            .to_owned()
        })?
        .to_string();
    // Set the actual bundle before the plugin attributes development builds to Terminal.
    static IDENTITY: OnceLock<Result<(), String>> = OnceLock::new();
    IDENTITY
        .get_or_init(|| notify_rust::set_application(&identifier).map_err(|e| e.to_string()))
        .clone()
        .map_err(|e| error(english, e))?;
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let (sender, receiver) = mpsc::channel();
    let callback = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // The notification center owns settings throughout this callback.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = sender.send(status);
    });
    center.getNotificationSettingsWithCompletionHandler(&callback);
    let status = receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|e| error(english, e))?;
    let denied = || {
        text(
            english,
            "系统设置未允许通知",
            "Notifications are not allowed in System Settings",
        )
        .to_owned()
    };
    if status == UNAuthorizationStatus::NotDetermined {
        if !request {
            return Err(text(
                english,
                "请先发送测试通知以授权",
                "Send a test notification to grant permission first",
            )
            .into());
        }
        let (sender, receiver) = mpsc::channel();
        let callback = RcBlock::new(move |granted: Bool, _| {
            let _ = sender.send(granted.as_bool());
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &callback,
        );
        return match receiver.recv_timeout(Duration::from_secs(30)) {
            Ok(true) => Ok(()),
            Ok(false) => Err(denied()),
            Err(e) => Err(error(english, e)),
        };
    }
    if matches!(
        status,
        UNAuthorizationStatus::Authorized | UNAuthorizationStatus::Provisional
    ) {
        Ok(())
    } else {
        Err(denied())
    }
}

#[cfg(windows)]
pub(super) fn check(app: &tauri::AppHandle, _request: bool, english: bool) -> Result<(), String> {
    use windows::UI::Notifications::{NotificationSetting, ToastNotificationManager};
    use windows::core::HSTRING;
    let id = identity(&app.config().identifier, crate::PREVIEW);
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(id))
        .map_err(|e| error(english, e))?;
    let setting = notifier.Setting().map_err(|e| error(english, e))?;
    if setting == NotificationSetting::Enabled {
        Ok(())
    } else {
        Err(format!(
            "{} ({})",
            text(
                english,
                "Windows 设置已关闭通知",
                "Notifications are disabled in Windows settings"
            ),
            setting.0
        ))
    }
}

/// The plugin cannot override its Windows AUMID. Use its native backend with an explicit
/// identity, including in development, and propagate failures instead of swallowing them.
#[cfg(windows)]
pub(super) fn send_windows(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    english: bool,
) -> Result<(), String> {
    notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .app_id(identity(&app.config().identifier, crate::PREVIEW))
        .show()
        .map(|_| ())
        .map_err(|e| error(english, e))
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(super) fn check(app: &tauri::AppHandle, _request: bool, english: bool) -> Result<(), String> {
    use tauri::plugin::PermissionState;
    use tauri_plugin_notification::NotificationExt;
    match app
        .notification()
        .permission_state()
        .map_err(|e| error(english, e))?
    {
        PermissionState::Granted => Ok(()),
        _ => Err(text(
            english,
            "系统未授予通知权限",
            "Notification permission has not been granted",
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn preview_has_its_own_windows_identity() {
        assert_eq!(super::identity("release", false), "release");
        assert_eq!(
            super::identity("release", true),
            "io.github.timwongup.agentnave.preview"
        );
    }

    #[test]
    fn native_errors_keep_details_and_follow_the_interface_language() {
        assert!(super::error(false, "AUMID not registered").starts_with("无法发送系统通知"));
        assert!(super::error(true, "AUMID not registered").contains("AUMID not registered"));
    }
}
