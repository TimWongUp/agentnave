//! Native permission checks and notification delivery.

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
    use std::sync::mpsc;
    use std::time::Duration;

    // Outside a bundle UNUserNotificationCenter raises an Objective-C exception.
    NSBundle::mainBundle().bundleIdentifier().ok_or_else(|| {
        text(
            english,
            "系统通知需要从 App 应用包运行",
            "Notifications require the bundled App",
        )
        .to_owned()
    })?;
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

#[cfg(target_os = "macos")]
mod macos {
    use block2::{DynBlock, RcBlock};
    use objc2::runtime::ProtocolObject;
    use objc2::{AnyThread, define_class, msg_send};
    use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString, NSUUID};
    use objc2_user_notifications::{
        UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
        UNNotificationRequest, UNNotificationSound, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };
    use std::sync::{Once, mpsc};
    use std::time::Duration;

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; the delegate has no state.
        #[unsafe(super = NSObject)]
        struct NotificationDelegate;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for NotificationDelegate {}

        // SAFETY: The method uses the protocol's declared signature.
        unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }
        }
    );

    /// Apple requires the delegate before launch finishes, on the main thread; without it a
    /// notification sent while the App is frontmost is never shown.
    pub(super) fn install() {
        static DELEGATE: Once = Once::new();
        DELEGATE.call_once(|| {
            // Outside a bundle UNUserNotificationCenter raises an Objective-C exception.
            if objc2_foundation::NSBundle::mainBundle()
                .bundleIdentifier()
                .is_none()
            {
                return;
            }
            let center = UNUserNotificationCenter::currentNotificationCenter();
            // The center's delegate is weak, so retain this stateless object for the process lifetime.
            let delegate: objc2::rc::Retained<NotificationDelegate> =
                unsafe { msg_send![NotificationDelegate::alloc(), init] };
            center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            let _ = objc2::rc::Retained::into_raw(delegate);
        });
    }

    pub(super) fn send(title: &str, body: &str, english: bool) -> Result<(), String> {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSUUID::UUID().UUIDString(),
            &content,
            None,
        );
        let (sender, receiver) = mpsc::channel();
        let callback = RcBlock::new(move |failure: *mut NSError| {
            // The error belongs to the notification center and is valid during the callback.
            let result = unsafe { failure.as_ref() }.map(|e| e.localizedDescription().to_string());
            let _ = sender.send(result);
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&callback));
        match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(None) => Ok(()),
            Ok(Some(detail)) => Err(super::error(english, detail)),
            Err(e) => Err(super::error(english, e)),
        }
    }
}

/// Call from App setup on the main thread.
pub(super) fn install() {
    #[cfg(target_os = "macos")]
    macos::install();
}

#[cfg(target_os = "macos")]
pub(super) fn send(
    _app: &tauri::AppHandle,
    title: &str,
    body: &str,
    english: bool,
) -> Result<(), String> {
    macos::send(title, body, english)
}

/// Use the native backend directly and propagate delivery errors to the caller.
#[cfg(not(target_os = "macos"))]
pub(super) fn send(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    english: bool,
) -> Result<(), String> {
    let mut notification = notify_rust::Notification::new();
    notification.summary(title).body(body);
    #[cfg(windows)]
    notification.app_id(identity(&app.config().identifier, crate::PREVIEW));
    #[cfg(not(windows))]
    let _ = app;
    notification
        .show()
        .map(|_| ())
        .map_err(|e| error(english, e))
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(super) fn check(_app: &tauri::AppHandle, _request: bool, _english: bool) -> Result<(), String> {
    // Desktop notification daemons report policy or transport failures during send.
    Ok(())
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
