//! Repository-level lock and per-worktree OS usage leases (`<common_dir>/agentnave/`).

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const LOCK_RETRY: Duration = Duration::from_millis(25);
const DELETE_PENDING_RETRY: Duration = Duration::from_millis(10);

/// Valid `wt-<12 lowercase hex>` identifier.
pub(crate) fn valid_isolation_id(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("wt-") else {
        return false;
    };
    hex.len() == 12
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn is_link(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    !plain_entry(&meta)
}

pub(crate) fn plain_entry(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    !meta.is_symlink()
}

pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (Ok(ma), Ok(mb)) = (fs::metadata(a), fs::metadata(b)) else {
            return false;
        };
        (ma.dev(), ma.ino()) == (mb.dev(), mb.ino())
    }
    #[cfg(not(unix))]
    {
        let (Ok(ca), Ok(cb)) = (fs::canonicalize(a), fs::canonicalize(b)) else {
            return false;
        };
        ca == cb
    }
}

pub(crate) fn is_same_or_subpath(child: &Path, parent: &Path) -> bool {
    let (Ok(parent_res), Ok(child_res)) = (fs::canonicalize(parent), fs::canonicalize(child))
    else {
        return false;
    };
    #[cfg(windows)]
    {
        let norm_parent = parent_res.to_string_lossy().to_lowercase();
        let norm_child = child_res.to_string_lossy().to_lowercase();
        if Path::new(&norm_child).starts_with(Path::new(&norm_parent)) {
            return true;
        }
    }
    #[cfg(not(windows))]
    if child_res.starts_with(&parent_res) {
        return true;
    }
    let parent_depth = parent_res.components().count();
    for ancestor in child_res.ancestors() {
        if ancestor.components().count() < parent_depth {
            break;
        }
        if same_dir(ancestor, &parent_res) {
            return true;
        }
    }
    false
}

fn check_safe_dir(path: &Path) -> Result<(), String> {
    if is_link(path) {
        return Err(format!("仓库锁路径存在不安全链接：{}", path.display()));
    }
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
        Ok(meta) if !meta.is_dir() => Err(format!("仓库状态路径不是目录：{}", path.display())),
        Ok(_) => Ok(()),
    }
}

fn check_safe_regular(path: &Path) -> Result<(), String> {
    if is_link(path) {
        return Err(format!("锁文件存在不安全链接：{}", path.display()));
    }
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
        Ok(meta) if !meta.is_file() => Err(format!("锁路径不是普通文件：{}", path.display())),
        Ok(_) => Ok(()),
    }
}

/// Check an existing disposable lease file; returns `Ok(false)` only once it is gone. A Windows
/// access denial is retried briefly (an owner may be unlinking it) and then fails closed.
fn check_lease_regular(path: &Path) -> Result<bool, String> {
    for attempt in 0..3 {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e)
                if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied && attempt < 2 =>
            {
                thread::sleep(DELETE_PENDING_RETRY);
            }
            Err(e) => return Err(e.to_string()),
            Ok(meta) if !plain_entry(&meta) => {
                return Err(format!("锁文件存在不安全链接：{}", path.display()));
            }
            Ok(meta) if !meta.is_file() => {
                return Err(format!("锁路径不是普通文件：{}", path.display()));
            }
            Ok(_) => return Ok(true),
        }
    }
    unreachable!("the last attempt always returns")
}

fn state_dir(repo_common_dir: &Path) -> Result<PathBuf, String> {
    if is_link(repo_common_dir) || !repo_common_dir.is_dir() {
        return Err(format!(
            "无效的 Git 公共目录：{}",
            repo_common_dir.display()
        ));
    }
    let dir = repo_common_dir.join("agentnave");
    check_safe_dir(&dir)?;
    Ok(dir)
}

fn leases_dir(repo_common_dir: &Path, isolation_id: &str) -> Result<PathBuf, String> {
    if !valid_isolation_id(isolation_id) {
        return Err(format!("无效的隔离目录 ID：{isolation_id}"));
    }
    let base = state_dir(repo_common_dir)?.join("leases");
    check_safe_dir(&base)?;
    let target = base.join(isolation_id);
    check_safe_dir(&target)?;
    Ok(target)
}

use super::git::{GitRunner, bytes_to_path, trim_line_end};
use crate::data_files::create_private_dir;

fn open_lock(path: &Path, create: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Match db.rs and provider_health.rs: share read and write, but not delete, while open.
        options.custom_flags(0x0020_0000).share_mode(3);
    }
    options.open(path)
}

/// A Windows lease file an owner may be unlinking: worth another look, never proof it is free.
fn maybe_delete_pending(err: &io::Error, create: bool) -> bool {
    !create && cfg!(windows) && err.kind() == io::ErrorKind::PermissionDenied
}

/// `Ok(None)` only when the file is gone; anything uncertain after the retries counts as held.
fn try_lock_verified(path: &Path, create: bool) -> Result<Option<File>, TryLockError> {
    for _ in 0..5 {
        let file = match open_lock(path, create) {
            Ok(file) => file,
            Err(e) if !create && e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) if maybe_delete_pending(&e, create) => {
                thread::sleep(DELETE_PENDING_RETRY);
                continue;
            }
            Err(e) => return Err(TryLockError::Error(e)),
        };
        let opened = match file.metadata() {
            Ok(meta) if meta.is_file() && plain_entry(&meta) => meta,
            Ok(_) => return Err(TryLockError::Error(io::Error::other("锁不是普通文件"))),
            Err(e) => return Err(TryLockError::Error(e)),
        };
        file.try_lock()?;
        let current = match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() && plain_entry(&meta) => meta,
            Ok(_) => return Err(TryLockError::Error(io::Error::other("锁不是普通文件"))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) if maybe_delete_pending(&e, create) => {
                thread::sleep(DELETE_PENDING_RETRY);
                continue;
            }
            Err(e) => return Err(TryLockError::Error(e)),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (opened.dev(), opened.ino()) != (current.dev(), current.ino()) {
                continue;
            }
        }
        #[cfg(not(unix))]
        let _ = (opened, current);
        return Ok(Some(file));
    }
    Err(TryLockError::WouldBlock)
}

/// Held repository lock; dropping releases the OS lock while leaving the inode in place.
pub(crate) struct RepoLock(#[allow(dead_code)] File);

/// Acquire `<common_dir>/agentnave/lock`, retrying until `timeout` expires.
pub(crate) fn acquire_repo_lock(
    repo_common_dir: &Path,
    timeout: Duration,
) -> Result<RepoLock, String> {
    let state = state_dir(repo_common_dir)?;
    create_private_dir(&state).map_err(|e| e.to_string())?;
    check_safe_dir(&state)?;
    let lock_path = state.join("lock");
    check_safe_regular(&lock_path)?;
    let deadline = Instant::now() + timeout;
    loop {
        check_safe_regular(&lock_path)?;
        match try_lock_verified(&lock_path, true) {
            Ok(Some(file)) => return Ok(RepoLock(file)),
            Ok(None) | Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "仓库隔离目录锁正忙，请稍后重试：{}",
                        repo_common_dir.display()
                    ));
                }
                thread::sleep(LOCK_RETRY);
            }
            Err(TryLockError::Error(e)) => return Err(e.to_string()),
        }
    }
}

fn release_and_remove(file: File, path: &Path) {
    #[cfg(unix)]
    {
        let _ = fs::remove_file(path);
        drop(file);
    }
    #[cfg(not(unix))]
    {
        let _ = file.unlock();
        drop(file);
        let _ = fs::remove_file(path);
    }
}

/// Check whether any live process holds a lease on `isolation_id` and reclaim stale lease files.
/// Caller must hold `acquire_repo_lock(repo_common_dir)`.
pub(crate) fn has_active_leases(
    repo_common_dir: &Path,
    isolation_id: &str,
) -> Result<bool, String> {
    let dir = leases_dir(repo_common_dir, isolation_id)?;
    let read_dir = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string()),
    };
    let mut entries: Vec<PathBuf> = read_dir
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for entry in entries {
        if entry.extension().and_then(|ext| ext.to_str()) != Some("lock") {
            return Err(format!("租约目录中存在异常文件：{}", entry.display()));
        }
        if !check_lease_regular(&entry)? {
            continue;
        }
        match try_lock_verified(&entry, false) {
            Ok(None) => continue,
            Ok(Some(file)) => release_and_remove(file, &entry),
            Err(TryLockError::WouldBlock) => return Ok(true),
            Err(TryLockError::Error(e)) => return Err(e.to_string()),
        }
    }
    Ok(false)
}

pub(crate) fn remove_lease_dir_locked(
    repo_common_dir: &Path,
    isolation_id: &str,
) -> Result<(), String> {
    let dir = leases_dir(repo_common_dir, isolation_id)?;
    if !dir.exists() {
        return Ok(());
    }
    if has_active_leases(repo_common_dir, isolation_id)? {
        return Err(format!("隔离目录 {isolation_id} 仍有活跃租约"));
    }
    let _ = fs::remove_dir(&dir);
    Ok(())
}

/// Process-held OS file lock keeping an isolated worktree alive while a derived terminal runs.
pub(crate) struct HeldLease {
    pub(crate) path: PathBuf,
    file: Option<File>,
}

impl Drop for HeldLease {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            release_and_remove(file, &self.path);
        }
    }
}

fn random_lease_name() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        bytes.copy_from_slice(&nanos.to_le_bytes());
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{hex}.lock")
}

/// Resolve the Git common directory of `worktree_cwd` without executing hooks/filters.
pub(crate) fn resolve_repo_common_dir(worktree_cwd: &Path) -> Result<PathBuf, String> {
    if is_link(worktree_cwd) || !worktree_cwd.is_dir() {
        return Err(format!("无效的隔离工作目录：{}", worktree_cwd.display()));
    }
    let git = GitRunner::new()?;
    let out = git.run(
        worktree_cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        &[0],
        &[],
    )?;
    let path = bytes_to_path(trim_line_end(&out.stdout));
    if is_link(&path) || !path.is_dir() {
        return Err(format!("无效的 Git 公共目录：{}", path.display()));
    }
    Ok(path)
}

/// Acquire an independent OS usage lease for `isolation_id` under the repository lock (ADR-0109).
pub(crate) fn acquire_lease(
    repo_common_dir: &Path,
    isolation_id: &str,
    timeout: Duration,
) -> Result<HeldLease, String> {
    let _repo_lock = acquire_repo_lock(repo_common_dir, timeout)?;
    let dir = leases_dir(repo_common_dir, isolation_id)?;
    create_private_dir(&dir).map_err(|e| e.to_string())?;
    check_safe_dir(&dir)?;
    for _ in 0..8 {
        let path = dir.join(random_lease_name());
        if path.exists() || is_link(&path) {
            continue;
        }
        match try_lock_verified(&path, true) {
            Ok(Some(file)) => {
                return Ok(HeldLease {
                    path,
                    file: Some(file),
                });
            }
            Ok(None) | Err(TryLockError::WouldBlock) => continue,
            Err(TryLockError::Error(e)) => return Err(e.to_string()),
        }
    }
    Err(format!("无法为隔离目录 {isolation_id} 分配租约锁文件"))
}
