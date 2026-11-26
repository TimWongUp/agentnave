//! Non-isolated working directory checks for the read-only ADR-0115 Git/PR status (ADR-0126).

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
}
