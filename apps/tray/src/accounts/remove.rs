//! Deleting a managed account's folder, which holds its sign-in and chats: only the folder the
//! App made for that id, and only after its real path has been checked.

use super::{Account, managed_root, real};
use std::fs;
use std::path::Path;

/// A plain folder: no link or junction stands in for it.
fn plain_dir(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    meta.is_dir() && !meta.is_symlink()
}

/// Whether `account`'s folder may be deleted: a managed one that is exactly
/// `<managed root>/<id>`, reached through no link, under a managed root that is no link either.
pub(super) fn deletable(account: &Account, home: &Path) -> Result<(), String> {
    if !account.managed {
        return Err("关联的外部目录不会被删除".into());
    }
    let root = managed_root(home);
    let expected = root.join(&account.id);
    if !plain_dir(&expected) || !plain_dir(&root) || !plain_dir(&home.join(super::MANAGED)) {
        return Err("账号目录不是 App 创建的普通文件夹，已保留".into());
    }
    if real(&account.dir) != real(&expected) || real(&expected) != real(&root).join(&account.id) {
        return Err("账号目录不在托管位置，已保留".into());
    }
    Ok(())
}

/// Deletes the folder of an account that passed `deletable`.
pub(super) fn delete(account: &Account, home: &Path) -> Result<(), String> {
    deletable(account, home)?;
    fs::remove_dir_all(managed_root(home).join(&account.id))
        .map_err(|error| format!("无法删除账号目录：{error}"))
}
