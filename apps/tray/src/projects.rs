//! The project a workspace belongs to, derived from its session records and the directories on
//! this machine. The service neither stores nor knows it; only the repository a directory
//! belonged to is remembered here, so a directory removed later still reaches it.
//!
//! Resolving touches the filesystem, so it happens on a background thread and the answer is
//! cached by working directory; until it arrives (or when it fails) a workspace is in "other".

mod auto;
mod classify;
mod env;
mod git;
mod memory;
mod path;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::mpsc::{Sender, channel};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use env::{Env, Stamp};
use memory::Memory;

/// The fixed key of the group for places that are not projects.
pub(crate) const OTHER: &str = "other";

/// A cached answer is used while it is served again in the background after this long, since
/// a directory can become a repository or disappear.
const REFRESH: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Project {
    /// The normalized full path; `OTHER` for no project.
    pub key: String,
    pub label: String,
    /// The path as shown, to tell projects with one name apart.
    pub path: String,
}

impl Project {
    pub(crate) fn other() -> Self {
        Self {
            key: OTHER.to_owned(),
            label: String::new(),
            path: String::new(),
        }
    }
}

struct Entry {
    project: Project,
    at: Instant,
    generation: u64,
}

#[derive(Default)]
struct State {
    cache: HashMap<String, Entry>,
    queued: HashSet<String>,
    generation: u64,
    worker: Option<Sender<Job>>,
    /// The origin working directories of the last lookup that had any, to spot a stale record.
    origins: Option<BTreeSet<String>>,
}

/// The work of the background thread: one directory, or the current set of origins.
enum Job {
    Resolve(String),
    Retain(BTreeSet<String>),
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);
static ENV: LazyLock<Mutex<Option<(Stamp, Env)>>> = LazyLock::new(Mutex::default);
static MEMORY: LazyLock<Mutex<Memory>> =
    LazyLock::new(|| Mutex::new(Memory::at(crate::preferences::dir().join(memory::FILE))));

fn shared() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

/// The records of directories that no longer exist, only touched on the background thread.
fn records() -> MutexGuard<'static, Memory> {
    MEMORY.lock().unwrap_or_else(|p| p.into_inner())
}

/// The projects of workspaces by their earliest working directory. Answers come from the cache;
/// directories not resolved yet are queued and count as "other" for now. Records of directories
/// that are no workspace's origin any more are dropped on the background thread.
pub(crate) fn lookup(origins: &BTreeMap<String, String>) -> BTreeMap<String, Project> {
    let mut state = shared();
    let generation = state.generation;
    let mut wanted = Vec::new();
    let found = origins
        .iter()
        .map(|(group, cwd)| {
            let project = match state.cache.get(cwd) {
                Some(entry) => {
                    if entry.generation != generation || entry.at.elapsed() > REFRESH {
                        wanted.push(cwd.clone());
                    }
                    entry.project.clone()
                }
                None => {
                    wanted.push(cwd.clone());
                    Project::other()
                }
            };
            (group.clone(), project)
        })
        .collect();
    for cwd in wanted {
        enqueue(&mut state, cwd);
    }
    // An empty list means it could not be read: nothing is forgotten then.
    let current: Option<BTreeSet<String>> =
        (!origins.is_empty()).then(|| origins.values().cloned().collect::<BTreeSet<_>>());
    if let Some(origins) = current.filter(|origins| state.origins.as_ref() != Some(origins))
        && send(&mut state, Job::Retain(origins.clone()))
    {
        state.origins = Some(origins);
    }
    found
}

fn enqueue(state: &mut State, cwd: String) {
    if !state.queued.insert(cwd.clone()) {
        return;
    }
    if !send(state, Job::Resolve(cwd.clone())) {
        state.queued.remove(&cwd);
    }
}

fn send(state: &mut State, job: Job) -> bool {
    if state.worker.is_none() {
        let (sender, receiver) = channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("agentnave-projects".into())
            .spawn(move || {
                for job in receiver {
                    work(job);
                }
            });
        if spawned.is_err() {
            return false;
        }
        state.worker = Some(sender);
    }
    state
        .worker
        .as_ref()
        .is_some_and(|worker| worker.send(job).is_ok())
}

fn work(job: Job) {
    match job {
        Job::Resolve(cwd) => {
            let (project, generation) = resolve(&cwd);
            let mut state = shared();
            state.queued.remove(&cwd);
            state.cache.insert(
                cwd,
                Entry {
                    project,
                    at: Instant::now(),
                    generation,
                },
            );
        }
        Job::Retain(origins) => records().retain(&origins),
    }
}

/// Resolve with the current places, reading them again when their files or variables changed.
fn resolve(cwd: &str) -> (Project, u64) {
    let now = env::stamp();
    let mut places = ENV.lock().unwrap_or_else(|p| p.into_inner());
    if places.as_ref().is_none_or(|(stamp, _)| *stamp != now) {
        if places.is_some() {
            shared().generation += 1;
        }
        *places = Some((now, Env::detect()));
    }
    let generation = shared().generation;
    let project = places.as_ref().map_or_else(Project::other, |(_, env)| {
        classify::classify(cwd, env, &mut records())
    });
    (project, generation)
}

#[cfg(test)]
mod tests;
