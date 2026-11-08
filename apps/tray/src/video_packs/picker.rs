use std::path::PathBuf;

#[cfg(target_os = "macos")]
pub(super) fn choose(app: &tauri::AppHandle) -> Result<Option<PathBuf>, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        // AppKit's modal panel and all borrowed Objective-C objects stay on the main thread.
        let chosen = unsafe {
            use objc2::{
                msg_send,
                runtime::{AnyClass, AnyObject, Bool},
            };
            let class = AnyClass::get(c"NSOpenPanel").expect("AppKit provides NSOpenPanel");
            let panel: *mut AnyObject = msg_send![class, openPanel];
            let _: () = msg_send![panel, setCanChooseFiles: Bool::NO];
            let _: () = msg_send![panel, setCanChooseDirectories: Bool::YES];
            let _: () = msg_send![panel, setAllowsMultipleSelection: Bool::NO];
            let result: isize = msg_send![panel, runModal];
            if result == 1 {
                let url: *mut AnyObject = msg_send![panel, URL];
                let path: *mut AnyObject = msg_send![url, path];
                let utf8: *const std::ffi::c_char = msg_send![path, UTF8String];
                if utf8.is_null() {
                    None
                } else {
                    Some(PathBuf::from(
                        std::ffi::CStr::from_ptr(utf8)
                            .to_string_lossy()
                            .into_owned(),
                    ))
                }
            } else {
                None
            }
        };
        let _ = sender.send(chosen);
    })
    .map_err(|error| error.to_string())?;
    receiver.recv().map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
pub(super) fn choose(_app: &tauri::AppHandle) -> Result<Option<PathBuf>, String> {
    Err("此平台暂不支持选择视频包文件夹".into())
}
