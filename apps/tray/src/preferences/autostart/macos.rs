use std::path::Path;

pub(super) fn repair_file(file: &Path, exe: &Path) -> Result<bool, String> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    let mut document = plist::Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|error| error.to_string())?;
    let arguments = document
        .as_dictionary_mut()
        .and_then(|document| document.get_mut("ProgramArguments"))
        .and_then(plist::Value::as_array_mut)
        .ok_or("开机启动文件缺少 ProgramArguments")?;
    let program = arguments.first_mut().ok_or("开机启动文件缺少可执行文件")?;
    let current = exe.to_str().ok_or("可执行文件路径无法转换为 Unicode")?;
    if program.as_string() == Some(current) {
        return Ok(false);
    }
    *program = plist::Value::String(current.into());
    let parent = file.parent().ok_or("开机启动文件没有父目录")?;
    let mut saved = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    document
        .to_writer_xml(saved.as_file_mut())
        .map_err(|error| error.to_string())?;
    saved
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    saved.persist(file).map_err(|error| error.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_the_app_repairs_only_its_enabled_launch_agent_and_keeps_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app.plist");
        let exe = Path::new("/Applications/New & Improved.app/Contents/MacOS/tray");
        assert!(!repair_file(&file, exe).unwrap());
        assert!(!file.exists());
        std::fs::write(&file, br#"<plist version="1.0"><dict>
            <key>ProgramArguments</key><array><string>/old/tray</string><string>--kept</string></array>
            <key>RunAtLoad</key><true/><key>Other</key><string>keep</string>
        </dict></plist>"#).unwrap();
        assert!(repair_file(&file, exe).unwrap());
        let document = plist::Value::from_file(&file).unwrap();
        let document = document.as_dictionary().unwrap();
        let arguments = document["ProgramArguments"].as_array().unwrap();
        assert_eq!(arguments[0].as_string(), exe.to_str());
        assert_eq!(arguments[1].as_string(), Some("--kept"));
        assert_eq!(document["Other"].as_string(), Some("keep"));
        assert!(!repair_file(&file, exe).unwrap());
    }
}
