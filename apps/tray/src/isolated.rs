//! The service's registry of isolated directories (`worktrees.db`): the App displays them and
//! deletes idle or unfinished ones under the service's repository lock and lease protocol
//! (ADR-0085, ADR-0098).

mod git;
mod leases;
mod release;
mod request;
mod state;
mod store;

use std::path::Path;

use serde_json::{Value, json};

pub(crate) use request::request;

/// One registered isolated directory that is past creation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dir {
    pub id: String,
    pub created: String,
    pub workspace: String,
    pub path: String,
    pub branch: String,
    /// Registered as usable; any other state is waiting to be dealt with.
    pub ready: bool,
    /// Active OS usage lease exists in `<common_dir>/agentnave/leases/<id>/`.
    pub leased: bool,
    /// Last failure reason when `state == 'needs_attention'`.
    pub attention: Option<String>,
}

/// Every registered directory; a missing, unreadable or newer database shows nothing.
pub(crate) fn read(root: &Path) -> Vec<Dir> {
    store::read_dirs(root).unwrap_or_default()
}

/// Whether `cwd` is `dir` or lies below it; symbolic links resolve when both still exist.
fn within(cwd: &str, dir: &str) -> bool {
    let (cwd, dir) = (Path::new(cwd), Path::new(dir));
    if cwd.starts_with(dir) {
        return true;
    }
    match (cwd.canonicalize(), dir.canonicalize()) {
        (Ok(cwd), Ok(dir)) => cwd.starts_with(&dir),
        _ => false,
    }
}

/// The call that asked for a directory, found in a workspace's task views.
#[derive(Clone, Debug)]
pub(crate) struct Builder {
    pub id: String,
    pub path: String,
    pub desk: String,
    pub text: String,
}

/// A workspace's directories for the office, those in use last. Each is in use (a running call
/// works inside it or an active lease exists), left over (usable but nobody works in it) or
/// unfinished (the last release did not complete).
pub(crate) fn classify(
    dirs: &[Dir],
    workspace: &str,
    running: &[String],
    builders: &[Builder],
) -> Vec<Value> {
    let mut items: Vec<(bool, Value)> = dirs
        .iter()
        .filter(|dir| dir.workspace == workspace)
        .map(|dir| {
            let using =
                dir.leased || (dir.ready && running.iter().any(|cwd| within(cwd, &dir.path)));
            let status = if using {
                "using"
            } else if dir.ready {
                "idle"
            } else {
                "unfinished"
            };
            let task = builders
                .iter()
                .find(|b| b.id == dir.id || b.path == dir.path)
                .map(|b| json!({"desk": b.desk, "text": b.text}));
            let item = json!({
                "id": dir.id,
                "path": dir.path,
                "branch": dir.branch,
                "status": status,
                "in_use": using,
                "attention": dir.attention,
                "created": dir.created,
                "task": task,
            });
            (using, item)
        })
        .collect();
    items.sort_by_key(|(using, _)| *using);
    items.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests;
