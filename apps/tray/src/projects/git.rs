//! The repository a directory belongs to, read from `.git` entries only: git is never run and
//! its configuration is never read.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::path::plain;

/// A `.git` file or `commondir` is one short line; anything longer is not trusted.
const LIMIT: u64 = 4096;

fn read_small(path: &Path) -> Option<String> {
    let mut text = String::new();
    File::open(path)
        .ok()?
        .take(LIMIT + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= LIMIT).then_some(text)
}

/// A path written in a git file: `/` and `\` both separate, a relative one starts at `base`.
fn resolve(base: &Path, written: &str) -> PathBuf {
    let path = PathBuf::from(written.replace('\\', "/"));
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

/// The main checkout of a linked worktree, whose `.git` file points at `<main>/.git/worktrees/x`
/// (that directory's `commondir` names `<main>/.git`). A submodule has no `commondir`, and a
/// broken pointer names nothing: both stand for `dir` itself.
fn linked(dir: &Path, file: &Path) -> Option<PathBuf> {
    let text = read_small(file)?;
    let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    let git_dir = resolve(dir, target);
    let common = read_small(&git_dir.join("commondir"))?;
    let common = plain(&fs::canonicalize(resolve(&git_dir, common.trim())).ok()?);
    if common.file_name()? != ".git" || !common.is_dir() {
        return None;
    }
    common.parent().map(Path::to_path_buf)
}

/// The project root above `start` (itself, when it does not exist): the nearest directory with
/// a `.git` entry, or the main checkout that entry belongs to.
pub(super) fn root(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let git = dir.join(".git");
        let Ok(meta) = fs::metadata(&git) else {
            continue;
        };
        if meta.is_dir() {
            return Some(dir.to_path_buf());
        }
        if meta.is_file() {
            return Some(linked(dir, &git).unwrap_or_else(|| dir.to_path_buf()));
        }
    }
    None
}
