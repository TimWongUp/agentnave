//! Independent inbox snapshot reader and unread-result badge/notification scanner (ADR-0114).
//!
//! Covers every bound in-app terminal (including derived terminals acting as a boss). Surfaces a
//! badge and at most one system notification per unread episode only after events dwell for 30s
//! while the boss has no active blocking `wait`, and revokes the badge as soon as `wait` starts
//! or collects the results. Never writes to any terminal PTY.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::Deserialize;
use sysinfo::Pid;

use super::{Terminals, links};

/// How long an unread event must dwell while `waiting == false` before reminding the user.
pub(super) const DWELL: Duration = Duration::from_secs(30);

#[derive(Clone, Deserialize)]
pub(super) struct Item {
    pub(super) key: String,
    #[cfg_attr(not(any(unix, test)), allow(dead_code))]
    pub(super) kind: String,
    #[cfg_attr(not(any(unix, test)), allow(dead_code))]
    pub(super) role: Option<String>,
    #[cfg_attr(not(any(unix, test)), allow(dead_code))]
    pub(super) status: Option<String>,
    /// Already reported by wait (a review awaiting decision), so not unread.
    #[serde(default)]
    pub(super) delivered: bool,
}

#[derive(Deserialize)]
struct BossSnapshot {
    waiting: bool,
    #[serde(default)]
    items: Vec<Item>,
    #[serde(default)]
    in_flight: u64,
    #[serde(default)]
    event_seq: Option<u64>,
    #[serde(default)]
    delivered_seq: Option<u64>,
}

impl BossSnapshot {
    fn unread_count(&self) -> usize {
        if self.event_seq.is_some() || self.delivered_seq.is_some() {
            let event_seq = self.event_seq.unwrap_or(0);
            let delivered_seq = self.delivered_seq.unwrap_or(0);
            if event_seq > delivered_seq {
                let fresh = self.items.iter().filter(|item| !item.delivered).count();
                if fresh > 0 {
                    fresh
                } else {
                    usize::try_from(event_seq - delivered_seq)
                        .unwrap_or(1)
                        .max(1)
                }
            } else {
                0
            }
        } else {
            self.items.len()
        }
    }
}

#[derive(Deserialize)]
struct InboxFile {
    version: u32,
    pid: u32,
    #[serde(default)]
    started_at: Option<f64>,
    #[serde(default)]
    bosses: HashMap<String, BossSnapshot>,
}

#[derive(Default)]
pub(super) struct RawBoss {
    pub(super) waiting: bool,
    pub(super) items: Vec<Item>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) in_flight: u64,
    pub(super) has_unread: bool,
    pub(super) unread_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UnreadNotice {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) boss_key: String,
    pub(super) count: usize,
    pub(super) derived: bool,
}

#[derive(Default)]
struct BossDwell {
    /// First observed time (while `!waiting`) for each currently unread item key.
    item_since: HashMap<String, Instant>,
    /// Fallback timer when `event_seq > delivered_seq` without item keys.
    seq_since: Option<Instant>,
    /// Prevents repeating system notifications during the same unread episode.
    notified: bool,
}

#[derive(Default)]
pub(super) struct Tracker {
    bosses: HashMap<String, BossDwell>,
}

fn started_at_matches(started_at: Option<f64>, process: &sysinfo::Process) -> bool {
    let Some(started) = started_at else {
        return true;
    };
    process.start_time() != 0 && (started - process.start_time() as f64).abs() <= 2.0
}

fn read_inbox(path: &Path, process: &sysinfo::Process, expected_pid: u32) -> Option<InboxFile> {
    if fs::symlink_metadata(path).ok()?.file_type().is_symlink() {
        return None;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let millis = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    let millis = u64::try_from(millis).ok()?;
    if process.start_time() == 0 || !links::same_process(process, Some(millis)) {
        return None;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let inbox: InboxFile = serde_json::from_slice(&bytes).ok()?;
    if inbox.version != 1
        || inbox.pid != expected_pid
        || !started_at_matches(inbox.started_at, process)
    {
        return None;
    }
    Some(inbox)
}

pub(super) fn inboxes(root: &Path) -> HashMap<String, RawBoss> {
    let dir = root.join("inbox");
    if dir.is_symlink() {
        return HashMap::new();
    }
    let mut files: Vec<(PathBuf, u32)> = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(pid) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if path.extension().is_none_or(|s| s != "json") {
            continue;
        }
        files.push((path, pid));
    }
    if files.is_empty() {
        return HashMap::new();
    }
    let pids: HashSet<u32> = files.iter().map(|(_, pid)| *pid).collect();
    let pids: Vec<u32> = pids.into_iter().collect();
    let system = links::snapshot(&pids);
    let mut bosses: HashMap<String, RawBoss> = HashMap::new();
    for (path, pid) in files {
        let Some(process) = system.process(Pid::from_u32(pid)) else {
            continue;
        };
        let Some(inbox) = read_inbox(&path, process, pid) else {
            continue;
        };
        for (key, boss) in inbox.bosses {
            let entry = bosses.entry(key).or_default();
            entry.waiting |= boss.waiting;
            entry.in_flight = entry.in_flight.saturating_add(boss.in_flight);
            let unread = boss.unread_count();
            if unread > 0 {
                entry.has_unread = true;
                entry.unread_count = entry.unread_count.saturating_add(unread);
                entry
                    .items
                    .extend(boss.items.into_iter().filter(|item| !item.delivered));
            }
        }
    }
    bosses
}

impl Tracker {
    pub(super) fn scan(
        &mut self,
        terminals: &Terminals,
        links: &HashMap<u64, String>,
        raw: &HashMap<String, RawBoss>,
        now: Instant,
        mut notify: impl FnMut(UnreadNotice),
    ) {
        let bound: Vec<(u64, String, bool)> = {
            let state = terminals.lock();
            state
                .open
                .iter()
                .filter(|r| r.session.exit().is_none())
                .filter_map(|r| Some((r.id, links.get(&r.id)?.clone(), r.derived.is_some())))
                .collect()
        };

        let mut bound_bosses: HashMap<&str, bool> = HashMap::new();
        for (_, boss_key, derived) in &bound {
            bound_bosses
                .entry(boss_key.as_str())
                .and_modify(|d| *d &= *derived)
                .or_insert(*derived);
        }

        // Drop dwell & notification state once a boss is unbound or has no unread events left.
        self.bosses.retain(|key, _| {
            bound_bosses.contains_key(key.as_str())
                && raw.get(key.as_str()).is_some_and(|b| b.has_unread)
        });

        let mut overdue_by_boss: HashMap<&str, usize> = HashMap::new();
        for (&boss_key, &derived) in &bound_bosses {
            let Some(boss) = raw.get(boss_key).filter(|b| b.has_unread) else {
                continue;
            };
            let dwell = self.bosses.entry(boss_key.to_owned()).or_default();
            let overdue = evaluate_boss(boss, dwell, now);
            overdue_by_boss.insert(boss_key, overdue);
            if overdue > 0 && !dwell.notified {
                dwell.notified = true;
                notify(UnreadNotice {
                    boss_key: boss_key.to_owned(),
                    count: overdue,
                    derived,
                });
            }
        }

        let pending_by_terminal: HashMap<u64, usize> = bound
            .iter()
            .map(|(id, boss_key, _)| {
                (
                    *id,
                    overdue_by_boss.get(boss_key.as_str()).copied().unwrap_or(0),
                )
            })
            .collect();

        for running in &mut terminals.lock().open {
            running.pending = pending_by_terminal.get(&running.id).copied().unwrap_or(0);
        }
    }
}

fn evaluate_boss(boss: &RawBoss, dwell: &mut BossDwell, now: Instant) -> usize {
    if boss.waiting {
        dwell.item_since.clear();
        dwell.seq_since = None;
        return 0;
    }
    if boss.items.is_empty() {
        dwell.item_since.clear();
        let since = *dwell.seq_since.get_or_insert(now);
        return if now.saturating_duration_since(since) >= DWELL {
            boss.unread_count.max(1)
        } else {
            0
        };
    }
    dwell.seq_since = None;
    let unique_keys: HashSet<&str> = boss.items.iter().map(|i| i.key.as_str()).collect();
    dwell
        .item_since
        .retain(|k, _| unique_keys.contains(k.as_str()));
    for &key in &unique_keys {
        dwell.item_since.entry(key.to_owned()).or_insert(now);
    }
    unique_keys
        .iter()
        .filter(|&&key| {
            dwell
                .item_since
                .get(key)
                .is_some_and(|&at| now.saturating_duration_since(at) >= DWELL)
        })
        .count()
}

#[cfg(test)]
mod tests;
