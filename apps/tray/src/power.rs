//! Whether the computer runs on battery, told to every page as `data-on-battery` so that
//! characters can stand still then.
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Manager};

static ON_BATTERY: AtomicBool = AtomicBool::new(false);
const CHECK_EVERY: Duration = Duration::from_secs(15);

/// The last state read, for pages opened before the next check.
pub fn on_battery() -> bool {
    ON_BATTERY.load(Ordering::Relaxed)
}

pub fn watch(app: AppHandle) {
    ON_BATTERY.store(read(), Ordering::Relaxed);
    thread::spawn(move || {
        loop {
            thread::sleep(CHECK_EVERY);
            let now = read();
            if ON_BATTERY.swap(now, Ordering::Relaxed) != now {
                let script = if now {
                    "document.documentElement.dataset.onBattery = 'true';"
                } else {
                    "delete document.documentElement.dataset.onBattery;"
                };
                for window in app.webview_windows().values() {
                    let _ = window.eval(script);
                }
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn read() -> bool {
    use std::ffi::{CStr, c_char, c_void};
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> *const c_void;
        fn IOPSGetProvidingPowerSourceType(snapshot: *const c_void) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
        fn CFStringGetCString(
            value: *const c_void,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
    }
    const UTF8: u32 = 0x0800_0100;
    // SAFETY: the snapshot is released once read; the source type belongs to the snapshot.
    unsafe {
        let snapshot = IOPSCopyPowerSourcesInfo();
        if snapshot.is_null() {
            return false;
        }
        let kind = IOPSGetProvidingPowerSourceType(snapshot);
        let mut buffer = [0 as c_char; 64];
        let battery = !kind.is_null()
            && CFStringGetCString(kind, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) != 0
            && CStr::from_ptr(buffer.as_ptr()).to_bytes() == b"Battery Power";
        CFRelease(snapshot);
        battery
    }
}

#[cfg(windows)]
fn read() -> bool {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: the call only fills the status we own.
    unsafe { GetSystemPowerStatus(&mut status) }.is_ok() && status.ACLineStatus == 0
}

#[cfg(not(any(target_os = "macos", windows)))]
fn read() -> bool {
    false
}
