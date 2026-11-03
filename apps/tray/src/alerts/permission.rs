//! The plugin's desktop `permission_state` always reports Granted. Query the native
//! settings instead, so the test button can report a known denial rather than success.

#[cfg(target_os = "macos")]
pub(super) fn check(_app: &tauri::AppHandle) -> Result<(), String> {
    use std::ptr::NonNull;
    use std::sync::OnceLock;
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSBundle;
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSettings,
        UNUserNotificationCenter,
    };

    // UNUserNotificationCenter raises an Objective-C exception outside an app bundle.
    // Such development executables are not the installed App's notification identity.
    let identifier = NSBundle::mainBundle()
        .bundleIdentifier()
        .ok_or("Notifications require the bundled App")?
        .to_string();
    // The plugin otherwise attributes development builds to Terminal. Set this once,
    // before its first send, so a bundled preview uses its own notification identity.
    static IDENTITY: OnceLock<Result<(), String>> = OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            notify_rust::set_application(&identifier).map_err(|error| error.to_string())
        })
        .clone()?;
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let (sender, receiver) = mpsc::channel();
    let callback = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // The notification center owns settings for the duration of its callback.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = sender.send(status);
    });
    center.getNotificationSettingsWithCompletionHandler(&callback);
    let status = receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    if status == UNAuthorizationStatus::NotDetermined {
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
            Ok(false) => Err("Notifications are not allowed in System Settings".into()),
            Err(error) => Err(error.to_string()),
        };
    }
    if matches!(
        status,
        UNAuthorizationStatus::Authorized | UNAuthorizationStatus::Provisional
    ) {
        Ok(())
    } else {
        Err("Notifications are not allowed in System Settings".into())
    }
}

#[cfg(windows)]
pub(super) fn check(app: &tauri::AppHandle) -> Result<(), String> {
    use windows::UI::Notifications::{NotificationSetting, ToastNotificationManager};
    use windows::core::HSTRING;

    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
        &app.config().identifier,
    ))
    .map_err(|e| e.to_string())?;
    let setting = notifier.Setting().map_err(|e| e.to_string())?;
    if setting == NotificationSetting::Enabled {
        Ok(())
    } else {
        Err(format!(
            "Notifications are disabled in Windows settings ({})",
            setting.0
        ))
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(super) fn check(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::plugin::PermissionState;
    use tauri_plugin_notification::NotificationExt;

    match app
        .notification()
        .permission_state()
        .map_err(|e| e.to_string())?
    {
        PermissionState::Granted => Ok(()),
        _ => Err("Notification permission has not been granted".into()),
    }
}
