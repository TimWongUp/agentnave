use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Maximum size of a `push/<pid>.json` snapshot shared between tray and service.
pub(crate) const PUSH_MAX_BYTES: usize = 256 * 1024;

/// Same rule as the Studio MCP server: `AGENTNAVE_DATA_DIR`, else `~/.agentnave`.
pub fn data_root() -> PathBuf {
    match std::env::var_os("AGENTNAVE_DATA_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::home_dir().unwrap_or_default().join(".agentnave"),
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub(crate) fn valid_uuid(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}

pub(crate) fn create_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path)
    }
}

fn write_atomic_bytes(root: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        create_private_dir(root)?;
        // Dropping the temporary file cleans it up on failure, leaving the old file intact.
        let mut temporary = tempfile::NamedTempFile::new_in(root)?;
        temporary.write_all(bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(root.join(name))?;
        Ok(())
    };
    write().map_err(|error| format!("无法保存 {name}：{error}"))
}

/// Replace `name` under `root` atomically: a unique sibling file, synced, then renamed.
pub(crate) fn write_atomic(root: &Path, name: &str, document: &Value) -> Result<(), String> {
    let mut bytes =
        serde_json::to_vec_pretty(document).map_err(|error| format!("无法保存 {name}：{error}"))?;
    bytes.push(b'\n');
    write_atomic_bytes(root, name, &bytes)
}

/// Replace `name` under `root` atomically using compact JSON so encoded size matches `to_vec`.
pub(crate) fn write_atomic_compact(
    root: &Path,
    name: &str,
    document: &Value,
) -> Result<(), String> {
    let bytes =
        serde_json::to_vec(document).map_err(|error| format!("无法保存 {name}：{error}"))?;
    write_atomic_bytes(root, name, &bytes)
}
