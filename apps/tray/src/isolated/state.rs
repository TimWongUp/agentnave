//! Worktree link validation, nested repository detection, and safe orphan tree removal.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::git::{GitRunner, bytes_to_path, decode_git_path};
use super::leases::{is_link, same_dir};

const MAX_FINGERPRINT_ENTRIES: usize = 200_000;

fn read_gitdir_pointer(dotgit: &Path, wt_path: &Path) -> Option<PathBuf> {
    let raw = fs::read_to_string(dotgit).ok()?;
    let first = raw.lines().next()?;
    let target = first.strip_prefix("gitdir:")?.trim();
    let pointed = PathBuf::from(target);
    Some(if pointed.is_absolute() {
        pointed
    } else {
        wt_path.join(pointed)
    })
}

pub(crate) fn worktree_link_valid(
    wt_path: &Path,
    admin_dir: Option<&str>,
    common_dir: &Path,
) -> bool {
    let dotgit = wt_path.join(".git");
    let Some(admin_str) = admin_dir else {
        return false;
    };
    if is_link(&dotgit) || !dotgit.is_file() {
        return false;
    }
    let Some(pointed) = read_gitdir_pointer(&dotgit, wt_path) else {
        return false;
    };
    let admin = Path::new(admin_str);
    if !pointed.is_dir() || !same_dir(&pointed, admin) {
        return false;
    }
    let Some(admin_parent) = admin.parent() else {
        return false;
    };
    if !same_dir(admin_parent, &common_dir.join("worktrees")) {
        return false;
    }
    let Ok(back) = fs::read_to_string(admin.join("gitdir")) else {
        return false;
    };
    Path::new(back.trim())
        .parent()
        .is_some_and(|parent| same_dir(parent, wt_path))
}

pub(crate) fn foreign_link_valid(wt_path: &Path) -> bool {
    let dotgit = wt_path.join(".git");
    if is_link(&dotgit) || !dotgit.is_file() {
        return false;
    }
    let Some(pointed) = read_gitdir_pointer(&dotgit, wt_path) else {
        return false;
    };
    if !pointed.is_dir()
        || pointed.parent().and_then(|p| p.file_name()) != Some("worktrees".as_ref())
    {
        return false;
    }
    let Ok(back) = fs::read_to_string(pointed.join("gitdir")) else {
        return false;
    };
    Path::new(back.trim())
        .parent()
        .is_some_and(|parent| same_dir(parent, wt_path))
}

fn is_bare_repo(names: &[String], dirs: &[String]) -> bool {
    names.iter().any(|n| n == "HEAD")
        && dirs.iter().any(|d| d == "objects")
        && dirs.iter().any(|d| d == "refs")
}

fn is_repo_dir(path: &Path) -> bool {
    fs::symlink_metadata(path.join(".git")).is_ok()
        || (path.join("HEAD").is_file()
            && path.join("objects").is_dir()
            && path.join("refs").is_dir())
}

fn rel_posix(wt_path: &Path, target: &Path) -> String {
    target
        .strip_prefix(wt_path)
        .unwrap_or(target)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn scan_ignored_subtree(wt_path: &Path, root: &Path, found: &mut BTreeSet<String>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                found.insert(rel_posix(wt_path, &dir));
                continue;
            }
        };
        let mut names = Vec::new();
        let mut subdirs = Vec::new();
        let mut has_dotgit = false;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if name == ".git" {
                has_dotgit = true;
                continue;
            }
            if is_link(&path) {
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                subdirs.push(path);
                names.push((name, true));
            } else {
                names.push((name, false));
            }
        }
        let file_names: Vec<String> = names
            .iter()
            .filter(|(_, is_dir)| !*is_dir)
            .map(|(n, _)| n.clone())
            .collect();
        let dir_names: Vec<String> = names
            .iter()
            .filter(|(_, is_dir)| *is_dir)
            .map(|(n, _)| n.clone())
            .collect();
        if has_dotgit || is_bare_repo(&file_names, &dir_names) {
            found.insert(rel_posix(wt_path, &dir));
        } else {
            stack.extend(subdirs);
        }
    }
}

fn scan_unreadable(wt_path: &Path, ignored: &BTreeSet<PathBuf>, found: &mut BTreeSet<String>) {
    let mut stack = vec![wt_path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                found.insert(rel_posix(wt_path, &dir));
                continue;
            }
        };
        for entry in entries.flatten() {
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            if is_link(&path) || ignored.contains(&path) {
                continue;
            }
            if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                stack.push(path);
            }
        }
    }
}

/// Detect gitlinks, untracked nested repositories, ignored repositories, and unreadable dirs.
pub(crate) fn nested_repositories(
    git: &GitRunner,
    wt_path: &Path,
    status_raw: &[u8],
) -> Result<Vec<String>, String> {
    let mut nested = BTreeSet::new();
    let listing = git.run(wt_path, &["ls-files", "-s", "-z"], &[0], &[])?;
    for item in listing.stdout.split(|&b| b == 0) {
        if item.starts_with(b"160000 ")
            && let Some(pos) = item.iter().position(|&b| b == b'\t')
        {
            nested.insert(decode_git_path(&item[pos + 1..]));
        }
    }
    for token in status_raw.split(|&b| b == 0) {
        if let Some(rest) = token.strip_prefix(b"? ")
            && rest.ends_with(b"/")
        {
            let trimmed = &rest[..rest.len() - 1];
            nested.insert(decode_git_path(trimmed));
        }
    }
    let ignored_list = git.run(
        wt_path,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
        &[0],
        &[],
    )?;
    let mut ignored = BTreeSet::new();
    let mut checked_parents = BTreeSet::new();
    for item in ignored_list.stdout.split(|&b| b == 0) {
        if !item.ends_with(b"/") {
            continue;
        }
        let rel = bytes_to_path(&item[..item.len() - 1]);
        let root = wt_path.join(rel);
        ignored.insert(root.clone());
        for parent in root.ancestors().skip(1) {
            if parent == wt_path || !checked_parents.insert(parent.to_path_buf()) {
                break;
            }
            if is_repo_dir(parent) {
                nested.insert(rel_posix(wt_path, parent));
            }
        }
        scan_ignored_subtree(wt_path, &root, &mut nested);
    }
    scan_unreadable(wt_path, &ignored, &mut nested);
    Ok(nested.into_iter().collect())
}

/// Digest of every entry under `root` without following links.
pub(crate) fn tree_fingerprint(root: &Path) -> Vec<u8> {
    let mut hasher = Sha256::new();
    let mut count = 0usize;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(read_dir) = fs::read_dir(&dir) else {
            hasher.update(format!("unreadable:{}\n", dir.display()).as_bytes());
            continue;
        };
        let mut entries: Vec<_> = read_dir.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            count += 1;
            if count > MAX_FINGERPRINT_ENTRIES {
                hasher.update(b"truncated\n");
                return hasher.finalize().to_vec();
            }
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                hasher.update(format!("missing:{}\n", path.display()).as_bytes());
                continue;
            };
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let stamp = entry_stat(&meta);
            hasher.update(format!("{}\t{stamp}\n", rel.display()).as_bytes());
            if meta.is_dir() && !is_link(&path) {
                pending.push(path);
            }
        }
    }
    hasher.finalize().to_vec()
}

pub(crate) fn entry_stat(meta: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mtime = i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
        format!("{}:{}:{}", meta.mode(), meta.size(), mtime)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        format!(
            "{}:{}:{}",
            meta.file_attributes(),
            meta.file_size(),
            meta.last_write_time()
        )
    }
}

/// Delete an orphan directory directly under `container` without following links.
pub(crate) fn remove_tree(path: &Path, container: &Path) -> Result<(), String> {
    if is_link(path) || !path.is_dir() {
        return Err(format!("拒绝删除 {}：不是普通目录", path.display()));
    }
    let Some(parent) = path.parent() else {
        return Err(format!("拒绝删除 {}", path.display()));
    };
    if is_link(parent) || !same_dir(parent, container) {
        return Err(format!(
            "拒绝删除 {}：不在 {} 目录内",
            path.display(),
            container.display()
        ));
    }
    make_tree_writable(path);
    fs::remove_dir_all(path)
        .map_err(|e| format!("无法删除目录 {}（{e}）；请手动清理", path.display()))?;
    if path.exists() {
        return Err(format!("无法删除目录 {}；请手动清理", path.display()));
    }
    Ok(())
}

fn make_tree_writable(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        set_writable(&dir);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_link(&path) {
                continue;
            }
            if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                stack.push(path);
            } else {
                set_writable(&path);
            }
        }
    }
}

// Making read-only files writable before deleting the tree is the point here.
#[allow(clippy::permissions_set_readonly_false)]
fn set_writable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::symlink_metadata(path) {
            let mode = meta.permissions().mode() | 0o700;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(meta) = fs::symlink_metadata(path) {
            let mut perms = meta.permissions();
            if perms.readonly() {
                perms.set_readonly(false);
                let _ = fs::set_permissions(path, perms);
            }
        }
    }
}
