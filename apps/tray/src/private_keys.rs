//! Helpers for files that hold secrets, shared by the voice and decision-model settings: writing a
//! file only the current user can read, and showing a saved key back in masked form.

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Replaces a file under `root` that only the current user may read; `*-keys.json` names also get
/// a restricted ACL on Windows.
pub(crate) fn write_private_json(root: &Path, name: &str, value: &Value) -> Result<(), String> {
    let save = || -> std::io::Result<()> {
        fs::create_dir_all(root)?;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(windows)]
        if name.ends_with("-keys.json") {
            restrict_windows_acl(file.path())?;
        }
        file.write_all(value.to_string().as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(root.join(name)).map_err(|e| e.error)?;
        Ok(())
    };
    save().map_err(|_| format!("无法保存 {name}"))
}

/// Windows has no file mode to copy: remove inherited access and grant the current user alone
/// before writing secret bytes into the temporary file.
#[cfg(windows)]
fn restrict_windows_acl(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;

    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.trim().is_empty() {
        return Err(std::io::Error::other("无法限制密钥文件权限"));
    }
    let icacls = std::env::var_os("SystemRoot")
        .map(|root| std::path::PathBuf::from(root).join(r"System32\icacls.exe"))
        .filter(|candidate| candidate.is_file())
        .unwrap_or_else(|| std::path::PathBuf::from("icacls.exe"));
    let run = |principal: &str| {
        std::process::Command::new(&icacls)
            .arg(path.as_os_str())
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{principal}:(F)"))
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status()
    };
    if let Ok(domain) = std::env::var("USERDOMAIN")
        && !domain.trim().is_empty()
        && run(&format!("{domain}\\{user}"))?.success()
    {
        return Ok(());
    }
    if run(&user)?.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("无法限制密钥文件权限"))
    }
}

/// How a saved API key is shown back: enough to recognize it, never enough to use it. A key of 16
/// characters or more keeps its first and last four, one of 8 to 15 only its last four, and a
/// shorter one nothing; an empty key has nothing to show.
pub(crate) fn mask_key(key: &str) -> String {
    let key = key.trim();
    let chars: Vec<char> = key.chars().collect();
    let tail = |from: usize| chars[from..].iter().collect::<String>();
    match chars.len() {
        0 => String::new(),
        16.. => format!(
            "{}…{}",
            chars[..4].iter().collect::<String>(),
            tail(chars.len() - 4)
        ),
        8.. => format!("••••{}", tail(chars.len() - 4)),
        _ => "••••".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::mask_key;

    #[test]
    fn a_key_is_masked_by_its_length_and_never_shown_whole() {
        let cases = [
            ("", ""),
            ("   ", ""),
            ("a", "••••"),
            ("1234567", "••••"),
            ("12345678", "••••5678"),
            ("AIza-secret-key", "••••-key"),
            ("123456789012345", "••••2345"),
            ("1234567890123456", "1234…3456"),
            ("  sk-proj-0123456789a8b  ", "sk-p…9a8b"),
            ("密钥密钥密钥密钥密钥密钥密钥密钥", "密钥密钥…密钥密钥"),
        ];
        for (key, masked) in cases {
            assert_eq!(mask_key(key), masked, "{key:?}");
            assert!(
                key.trim().is_empty() || !masked.contains(key.trim()),
                "{key:?}"
            );
        }
    }
}
