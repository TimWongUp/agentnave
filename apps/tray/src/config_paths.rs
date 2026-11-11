use std::path::Path;

pub(crate) fn safe_path(path: &Path, home: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("write_failed:配置路径必须是无父目录跳转的绝对路径".into());
    }
    for ancestor in path.ancestors() {
        if ancestor != home && ancestor.is_symlink() {
            return Err("symlink_path:配置路径经过符号链接，请手动处理".into());
        }
    }
    Ok(())
}

pub(crate) fn message(error: &str) -> &str {
    error_parts(error, "takeover_failed").1
}

pub(crate) fn error_parts<'a>(error: &'a str, fallback: &'a str) -> (&'a str, &'a str) {
    error
        .split_once(':')
        .filter(|(c, _)| {
            [
                "parse_failed",
                "command_missing",
                "symlink_path",
                "root_unconfirmed",
                "conflict",
                "write_failed",
                "rolled_back",
                "takeover_failed",
            ]
            .contains(c)
        })
        .unwrap_or((fallback, error))
}
