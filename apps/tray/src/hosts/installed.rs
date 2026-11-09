//! Desktop installations are evidence about readers, separate from a CLI on PATH.
use super::*;

pub(super) fn desktop(context: &Context, boss: &str) -> Option<(&'static str, Option<PathBuf>)> {
    #[cfg(target_os = "macos")]
    {
        let name = match boss {
            "codex" => &["ChatGPT.app", "Codex.app"][..],
            "claude" => &["Claude.app"][..],
            "workbuddy" => &["WorkBuddy.app"][..],
            "antigravity" => &["Antigravity.app"][..],
            "deepseek" => &["DeepSeek Harness.app"][..],
            "devin" => &["Devin.app"][..],
            "zcode" => &["ZCode.app"][..],
            _ => &[],
        };
        if let Some(path) = context
            .applications
            .iter()
            .flat_map(|base| name.iter().map(move |name| base.join(name)))
            .find(|path| {
                path.is_dir()
                    && (boss != "codex"
                        || path.file_name().is_none_or(|name| name != "ChatGPT.app")
                        || codex_bundle(path))
            })
        {
            return Some(("app", Some(path)));
        }
    }
    #[cfg(windows)]
    if let Some(found) = windows(context, boss) {
        return Some(found);
    }
    #[cfg(target_os = "linux")]
    if boss == "codex" {
        let path = PathBuf::from("/usr/lib/chatgpt");
        if path.join("resources/linux-package-metadata.json").is_file() {
            return Some(("package", Some(path)));
        }
    }
    let _ = context;
    None
}

#[cfg(target_os = "macos")]
fn codex_bundle(path: &Path) -> bool {
    plist::Value::from_file(path.join("Contents/Info.plist"))
        .ok()
        .and_then(|value| {
            value
                .as_dictionary()?
                .get("CFBundleIdentifier")?
                .as_string()
                .map(str::to_owned)
        })
        .is_some_and(|id| id == "com.openai.codex")
}

#[cfg(windows)]
fn windows(context: &Context, boss: &str) -> Option<(&'static str, Option<PathBuf>)> {
    if boss == "codex" && store_package("OpenAI.Codex_", Some("2p2nqsd0c76g0")) {
        return Some(("store", None));
    }
    if boss == "claude" {
        let local = roots::env_value(&context.env, "LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| context.home.join("AppData/Local"));
        let squirrel = local.join("AnthropicClaude/claude.exe");
        if squirrel.is_file() {
            return Some(("app", Some(squirrel)));
        }
        if store_package("Claude_", None) || store_package("AnthropicPBC.Claude_", None) {
            return Some(("store", None));
        }
    }
    if boss == "workbuddy" {
        return crate::detect::uninstall_icon("WorkBuddy").map(|path| ("app", Some(path)));
    }
    if boss == "deepseek" {
        if let Some(path) = context
            .applications
            .iter()
            .map(|dir| dir.join("DeepSeek Harness/DeepSeek Harness.exe"))
            .find(|path| path.is_file())
        {
            return Some(("app", Some(path)));
        }
        return crate::detect::uninstall_icon("DeepSeek Harness").map(|path| ("app", Some(path)));
    }
    None
}

#[cfg(windows)]
fn store_package(prefix: &str, publisher: Option<&str>) -> bool {
    let key = r"HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";
    crate::detect::store_packages(key)
        .iter()
        .any(|name| main_store_package(name, prefix, publisher))
}

#[cfg(any(test, windows))]
fn main_store_package(name: &str, prefix: &str, publisher: Option<&str>) -> bool {
    let Some(rest) = name.strip_prefix(prefix) else {
        return false;
    };
    let parts: Vec<_> = rest.split('_').collect();
    parts.len() == 4
        && !parts[0].is_empty()
        && !parts[1].is_empty()
        && parts[2].is_empty()
        && publisher.is_none_or(|publisher| parts[3] == publisher)
}

#[cfg(test)]
mod tests {
    use super::main_store_package;

    #[test]
    fn store_detection_rejects_resource_packages_and_other_publishers() {
        assert!(main_store_package(
            "OpenAI.Codex_1.2.3.4_x64__pub",
            "OpenAI.Codex_",
            Some("pub")
        ));
        assert!(!main_store_package(
            "OpenAI.Codex_1.2.3.4_x64_scale-125_pub",
            "OpenAI.Codex_",
            Some("pub")
        ));
        assert!(!main_store_package(
            "OpenAI.Codex_1.2.3.4_x64__other",
            "OpenAI.Codex_",
            Some("pub")
        ));
    }
}
