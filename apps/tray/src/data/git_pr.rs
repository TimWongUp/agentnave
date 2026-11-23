//! Non-isolated working directory checks for ADR-0115 Git/PR status and write guards.

use std::path::{Path, PathBuf};

use super::Reader;

/// Whether `candidate_cwd` is inside `target_dir` (same dir or subdirectory) without falling into
/// a separate isolated worktree nested under `target_dir`.
pub(crate) fn is_non_isolated_in_dir(
    candidate_cwd: &Path,
    target_dir: &Path,
    isolated_dirs: &[PathBuf],
) -> bool {
    if !crate::isolated::leases::is_same_or_subpath(candidate_cwd, target_dir) {
        return false;
    }
    let container = target_dir.join(crate::isolated::git::WORKTREES_DIR_NAME);
    if crate::isolated::leases::is_same_or_subpath(candidate_cwd, &container) {
        return false;
    }
    !isolated_dirs.iter().any(|iso| {
        !crate::isolated::leases::is_same_or_subpath(target_dir, iso)
            && crate::isolated::leases::is_same_or_subpath(candidate_cwd, iso)
    })
}

impl Reader {
    /// All currently running task and meeting-seat working directories across snapshots.
    pub(crate) fn running_cwds(&self) -> Vec<PathBuf> {
        let database = crate::db::read(&self.root);
        let (_, snapshots) = self.scan_with(&database);
        snapshots
            .iter()
            .flat_map(|snap| crate::stage::running_in(&snap.view["director"]))
            .map(PathBuf::from)
            .collect()
    }

    /// Whether any running task or meeting seat currently works in `dir` or a subdirectory
    /// (excluding separate isolated worktrees nested under `dir`).
    pub(crate) fn has_running_non_isolated_in_dir(&self, dir: &Path) -> bool {
        let isolated_dirs: Vec<PathBuf> = crate::isolated::read(&self.root)
            .into_iter()
            .map(|d| PathBuf::from(d.path))
            .collect();
        self.running_cwds()
            .iter()
            .any(|cwd| is_non_isolated_in_dir(cwd, dir, &isolated_dirs))
    }
}
