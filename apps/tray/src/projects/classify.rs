//! The rules that turn a working directory into a project (see CONTEXT.md, "项目").

use std::path::Path;

use super::env::Env;
use super::memory::Memory;
use super::path::{self, canonical};
use super::{OTHER, Project};

/// Where `cwd` is grouped. Only the record of past answers is written, and no process runs.
pub(super) fn classify(cwd: &str, env: &Env, memory: &mut Memory) -> Project {
    let Some(dir) = (!cwd.trim().is_empty())
        .then(|| canonical(Path::new(cwd.trim())))
        .flatten()
    else {
        return Project::other();
    };
    // Scratch places win even when a repository sits inside them.
    if env.is_scratch(&dir) {
        return Project::other();
    }
    let exists = dir.is_dir();
    // A directory removed since it was last seen (an agent worktree) is still the repository it
    // belonged to then.
    if !exists && let Some(root) = memory.root(&dir) {
        return project(&root, env);
    }
    // A repository at the home, desktop or downloads folder (dotfiles) does not make everything
    // below it one project.
    if let Some(root) = super::git::root(&dir).filter(|root| !env.is_bare(root)) {
        if exists {
            memory.remember(&dir, &root);
        }
        return project(&root, env);
    }
    if env.is_auto(&dir) || env.is_bare(&dir) {
        return Project::other();
    }
    project(&dir, env)
}

fn project(root: &Path, env: &Env) -> Project {
    let key = path::key(root);
    if key == OTHER {
        return Project::other();
    }
    let label = root
        .file_name()
        .map_or_else(|| root.to_string_lossy(), |name| name.to_string_lossy())
        .into_owned();
    let shown = match env
        .home
        .as_deref()
        .and_then(|home| root.strip_prefix(home).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.to_string_lossy().replace('\\', "/")),
        None => root.to_string_lossy().into_owned(),
    };
    Project {
        key,
        label,
        path: shown,
    }
}
