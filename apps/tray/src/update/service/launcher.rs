//! The path hosts trust never changes; exec leaves the versioned relay as their direct child.
use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path};

pub(super) fn write(base: &Path, target: &Path) -> Result<(), String> {
    let quoted = target.to_string_lossy().replace('\'', "'\"'\"'");
    let script = format!("#!/bin/sh\nexec '{quoted}' \"$@\"\n");
    replace(base, script.as_bytes())
}

pub(super) fn replace(base: &Path, script: &[u8]) -> Result<(), String> {
    let directory = base.join("bin");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join("agentnave-mcp");
    if fs::read(&path).is_ok_and(|bytes| bytes == script)
        && fs::metadata(&path).is_ok_and(|meta| meta.permissions().mode() & 0o777 == 0o755)
    {
        return Ok(());
    }
    let mut staging = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
    staging.write_all(script).map_err(|e| e.to_string())?;
    staging
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    staging.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests;
