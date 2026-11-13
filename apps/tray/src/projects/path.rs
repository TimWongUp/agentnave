//! Path normalization for project keys: no process is started and nothing is written.

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Windows spellings of one place: `\\?\C:\x` and `\\?\UNC\srv\share\x` become the plain forms
/// and the drive letter is upper case. Other text is returned unchanged.
pub(super) fn plain_windows(text: &str) -> String {
    let rest = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        return text.to_owned();
    };
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            format!("{}{}", letter.to_ascii_uppercase(), &rest[1..])
        }
        _ => rest,
    }
}

/// The path without Windows verbatim prefixes.
pub(super) fn plain(path: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(plain_windows(&path.to_string_lossy()))
    } else {
        path.to_path_buf()
    }
}

/// `.` and `..` resolved by name only.
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// macOS keeps some top-level folders as links into `/private`; a path that does not exist yet
/// cannot be resolved, so the fixed aliases are applied by name.
const MACOS_ALIASES: &[(&str, &str)] = &[
    ("/tmp", "/private/tmp"),
    ("/var", "/private/var"),
    ("/etc", "/private/etc"),
];

pub(super) fn align_with(path: PathBuf, aliases: &[(&str, &str)]) -> PathBuf {
    for (from, to) in aliases {
        if let Ok(rest) = path.strip_prefix(from) {
            return Path::new(to).join(rest);
        }
    }
    path
}

/// An absolute path in its one spelling: the longest existing ancestor resolved, the rest
/// appended by name. A relative path (or one that is not absolute on this platform) has none.
pub(super) fn canonical(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let cleaned = clean(path);
    let mut rest: Vec<OsString> = Vec::new();
    let mut current = cleaned.as_path();
    loop {
        if let Ok(base) = fs::canonicalize(current) {
            let mut out = plain(&base);
            out.extend(rest.iter().rev());
            let aliases = if cfg!(target_os = "macos") {
                MACOS_ALIASES
            } else {
                &[]
            };
            return Some(align_with(out, aliases));
        }
        rest.push(current.file_name()?.to_owned());
        current = current.parent()?;
    }
}

/// The form two paths are compared in: Windows ignores case, other systems do not.
pub(super) fn comparable(path: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    } else {
        path.to_path_buf()
    }
}

/// Whether `path` is `root` or lies below it, by whole components.
pub(super) fn within(path: &Path, root: &Path) -> bool {
    comparable(path).starts_with(comparable(root))
}

pub(super) fn same(left: &Path, right: &Path) -> bool {
    comparable(left) == comparable(right)
}

/// Whether the path is a volume root or has no parent to speak of.
pub(super) fn too_broad(path: &Path, home: Option<&Path>) -> bool {
    path.parent().is_none() || home.is_some_and(|home| same(path, home))
}

/// The key of a project root: its whole normalized path.
pub(super) fn key(path: &Path) -> String {
    comparable(path).to_string_lossy().into_owned()
}
