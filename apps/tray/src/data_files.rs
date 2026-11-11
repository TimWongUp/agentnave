use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

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

/// Replace `name` under `root` atomically: a unique sibling file, synced, then renamed.
pub(crate) fn write_atomic(root: &Path, name: &str, document: &Value) -> Result<(), String> {
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(root)?;
        // Dropping the temporary file cleans it up on failure, leaving the old file intact.
        let mut temporary = tempfile::NamedTempFile::new_in(root)?;
        temporary.write_all(&serde_json::to_vec_pretty(document)?)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist(root.join(name))?;
        Ok(())
    };
    write().map_err(|error| format!("无法保存 {name}：{error}"))
}
