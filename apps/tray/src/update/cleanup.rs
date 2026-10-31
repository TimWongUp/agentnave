//! Conservative collection of App-owned environments with runtime leases.
use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

const GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Only code that takes the lifetime lease may opt into collection. Legacy installs stay.
pub(super) fn register(env: &Path) {
    let Some(metadata) = super::installed_metadata(env) else {
        return;
    };
    if !metadata
        .parent()
        .is_some_and(|site| site.join("agentnave/service_lease.py").is_file())
    {
        return;
    }
    let Some(root) = env.parent().and_then(Path::parent) else {
        return;
    };
    if root
        .parent()
        .and_then(Path::file_name)
        .is_none_or(|name| name != "service")
    {
        return;
    }
    let leases = root.parent().unwrap().join(".leases");
    if fs::create_dir_all(&leases).is_err() {
        return;
    }
    // Do not truncate an existing inode: a relay may already hold its lock.
    if fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(leases.join(format!(
            "{}.lock",
            root.file_name().unwrap().to_string_lossy()
        )))
        .is_ok()
    {
        let _ = fs::write(root.join("gc-v1"), b"1");
    }
}

/// Called after host refresh. Missing or ambiguous configuration means no collection at all.
/// Keep active, the newest rollback, and any version mentioned anywhere in known config files.
pub(crate) fn prune(app: &Path, configurations: Option<Vec<String>>) {
    let Some(configurations) = configurations else {
        return;
    };
    let Some(active) = super::active_version(app) else {
        return;
    };
    let base = app.join("service");
    let Ok(entries) = fs::read_dir(&base) else {
        return;
    };
    let mut versions: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            let parts: Vec<_> = name.split('.').collect();
            (parts.len() == 3
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
                && entry.file_type().ok()?.is_dir())
            .then_some(name)
        })
        .collect();
    versions.sort_by_key(|version| super::parse(version));
    let rollback = versions
        .iter()
        .rev()
        .find(|v| super::parse(v) < super::parse(&active))
        .cloned();
    let configurations: Vec<_> = configurations.iter().map(|s| normalized(s)).collect();
    for version in versions {
        if version == active
            || Some(&version) == rollback.as_ref()
            || super::parse(&version) >= super::parse(&active)
            || configurations
                .iter()
                .any(|text| text.contains(&format!("/service/{version}/")))
        {
            continue;
        }
        let root = base.join(&version);
        let marker = root.join("gc-v1");
        let eligible = fs::read(&marker).is_ok_and(|value| value == b"1")
            && marker
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| SystemTime::now().duration_since(at).ok())
                .is_some_and(|age| age >= GRACE);
        if !eligible || !plain_tree(&root) {
            continue;
        }
        let Ok(lease) = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(base.join(".leases").join(format!("{version}.lock")))
        else {
            continue;
        };
        if lease.try_lock().is_err() {
            continue;
        }
        // Recheck the pointer under the lease. Move out of the launchable namespace before
        // releasing it: a stale launcher cannot acquire a fresh lease while deletion proceeds.
        if super::active_version(app).as_deref() != Some(&active) {
            return;
        }
        let retired = base.join(format!(".retired-{version}-{}", std::process::id()));
        if retired.exists() {
            continue;
        }
        if let Err(error) = fs::rename(&root, &retired) {
            #[cfg(test)]
            eprintln!("retire {}: {error}", root.display());
            #[cfg(not(test))]
            let _ = error;
            continue;
        }
        drop(lease);
        let _ = fs::remove_dir_all(retired);
    }
}

fn normalized(text: &str) -> String {
    let text = text.replace("\\\\", "\\").replace('\\', "/");
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// Never recurse through a symlink or Windows junction, including links inside an install.
fn plain_tree(root: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(root) else {
        return false;
    };
    if meta.is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    if !meta.is_dir() {
        return true;
    }
    fs::read_dir(root).is_ok_and(|entries| {
        entries
            .into_iter()
            .all(|e| e.is_ok_and(|e| plain_tree(&e.path())))
    })
}

#[cfg(test)]
mod tests;
