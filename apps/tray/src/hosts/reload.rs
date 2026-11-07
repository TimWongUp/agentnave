//! Whether hosts still run an MCP entry from before its last change (ADR-0022).
use super::*;

/// A receipt this old with no refresh belongs to a relay that exited without removing it.
const LINGER_SECONDS: u64 = 150;
/// Relays prune receipts, ended ones included, after an hour without a write.
const ENDED_SECONDS: u64 = 3600;

#[derive(Default)]
struct Receipts {
    live: Vec<Value>,
    live_pids: Vec<Option<u32>>,
    lingering: bool,
    lingering_before_change: bool,
    ended: Vec<Value>,
}
struct Install {
    version: Option<String>,
}

impl Hosts {
    /// Only the MCP entries a relay starts from matter. Agents read the Skill from disk on use,
    /// and release numbers do not change what a connection runs.
    pub(super) fn reload_token(&self, host: &Host) -> Option<String> {
        let mut entries = Vec::new();
        for path in &host.user_layers {
            let document = host.read_at(path).ok()?;
            if let Some(entry) = document.entry {
                if !managed_entry(host, &entry) {
                    return None;
                }
                entries.push(json!({"path":path,"entry":entry}));
            }
        }
        if entries.is_empty() {
            return None;
        }
        Some(skill::hash(
            &serde_json::to_vec(&json!({"entries":entries})).ok()?,
        ))
    }
    pub(super) fn reload_state(&self, host: &Host, token: &str) -> Value {
        // Records from before the MCP-only token (v4) describe another token format.
        let record = |name: &str| {
            let value = &self.settings[host.id][name];
            if value["v"] == RELOAD_FORMAT {
                value
            } else {
                &Value::Null
            }
        };
        let (confirmed, changed, observed) = (
            record("reload_confirmed"),
            record("reload_changed"),
            record("reload_observed"),
        );
        let manual = confirmed["token"] == token;
        let pending = !manual
            && ((changed["token"] == token)
                || (confirmed["token"].is_string() && confirmed["token"] != token)
                || (observed["token"].is_string() && observed["token"] != token));
        let now = SystemTime::now();
        let changed_at = changed["at_ms"]
            .as_u64()
            .filter(|_| changed["token"] == token);
        let receipts = self.receipts(host, now, changed["at_ms"].as_u64());
        let install = self.install();
        let base = if pending {
            ("required", "config")
        } else if manual {
            ("confirmed", "connection")
        } else {
            ("unknown", "connection")
        };
        let (state, reason) = if !receipts.live.is_empty() {
            let newly_connected = !receipts.lingering_before_change
                && changed_at.is_some_and(|at| {
                    receipts
                        .live
                        .iter()
                        .all(|c| c["started_ms"].as_u64().is_some_and(|start| start > at))
                });
            live_state(&receipts.live, &install, !pending || newly_connected, base)
        } else if receipts.proves_reload(changed_at, &install, now) {
            ("current", "connection")
        } else if receipts.lingering {
            base
        } else {
            self.idle_state(host, pending, changed)
        };
        let copies: Vec<_> = self
            // The last recorded change, as idle_state judges an old process by it too.
            .reload_copies(host, reason, &receipts, &install, changed["at_ms"].as_u64())
            .into_iter()
            .map(processes::Copy::into_json)
            .collect();
        json!({"state":state,"reason":reason,"token":token,"connections":receipts.live.len(),"confirmed_at":confirmed["at"],"copies":copies})
    }
    fn reload_copies(
        &self,
        host: &Host,
        reason: &str,
        receipts: &Receipts,
        install: &Install,
        changed_at: Option<u64>,
    ) -> Vec<processes::Copy> {
        let mut probe = self.processes.lock().unwrap_or_else(|p| p.into_inner());
        if reason == "host_running" {
            return probe.old_copies(
                &self.context,
                trae::process_id(host),
                changed_at.unwrap_or(0),
            );
        }
        let mut copies: BTreeMap<&str, processes::Copy> = BTreeMap::new();
        if matches!(reason, "stale" | "tools" | "config") {
            for (receipt, pid) in receipts.live.iter().zip(&receipts.live_pids) {
                // A connection that started before the config change still runs the old entry.
                let old_config = reason == "config"
                    && changed_at.is_some_and(|at| {
                        receipt["started_ms"]
                            .as_u64()
                            .is_some_and(|start| start <= at)
                    });
                if !old_config && !stale(receipt, install) && !mismatched_tools(receipt) {
                    continue;
                }
                if let Some(pid) = pid
                    && let Some(copy) =
                        probe.receipt_copy(&self.context, trae::process_id(host), *pid)
                    && copies
                        .get(copy.kind)
                        .is_none_or(|old| copy.since < old.since)
                {
                    copies.insert(copy.kind, copy);
                }
            }
        }
        copies.into_values().collect()
    }
    /// Live receipts refresh every five seconds; a relay swapping to a new install may pause
    /// that for up to 105 seconds, so only a longer silence counts as no connection at all.
    /// A relay whose host closed normally marks its receipt ended and keeps it as proof.
    fn receipts(&self, host: &Host, now: SystemTime, changed_at: Option<u64>) -> Receipts {
        let mut receipts = Receipts::default();
        let dir = self
            .connection_root(host)
            .map(|root| root.join("connections"));
        for entry in dir
            .into_iter()
            .flat_map(|dir| fs::read_dir(dir).into_iter().flatten().flatten())
        {
            let Some((age, modified, value)) = read_receipt(&entry, now, host.boss) else {
                continue;
            };
            if host.boss == "trae" && !self.trae_receipt_matches(host, &entry.path()) {
                continue;
            }
            if !value["ended_ms"].is_null() {
                receipts.ended.push(value);
            } else if age <= LINGER_SECONDS
                && self.receipt_process_exited(&entry.path(), &value, modified)
            {
                continue;
            } else if age <= 20 {
                receipts.live_pids.push(receipt_pid(&entry.path()));
                receipts.live.push(value);
            } else if age <= LINGER_SECONDS {
                receipts.lingering = true;
                // A paused receipt that started after the change still runs the current entry;
                // one paused since before it, or with an unreadable start time, may not.
                receipts.lingering_before_change |= !value["started_ms"]
                    .as_u64()
                    .is_some_and(|start| changed_at.is_some_and(|at| start > at));
            }
        }
        receipts
    }
    fn trae_receipt_matches(&self, host: &Host, path: &Path) -> bool {
        receipt_pid(path).is_some_and(|pid| {
            self.processes
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .receipt_copy(&self.context, host.id, pid)
                .is_some()
        })
    }
    /// A missing or reused PID only excludes an old blocker; it never proves tools loaded.
    fn receipt_process_exited(&self, path: &Path, value: &Value, modified: SystemTime) -> bool {
        let Some(pid) = receipt_pid(path) else {
            return false;
        };
        matches!(
            self.processes
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .receipt_process(&self.context, pid, value["started_ms"].as_u64(), modified),
            processes::State::Stopped
        )
    }
    fn install(&self) -> Install {
        let env = crate::update::active_tools(&self.context.app)
            .or_else(|| self.tool_dirs().map(|dirs| dirs.tools.join("agentnave")));
        Install {
            version: env.as_deref().and_then(crate::update::installed_version),
        }
    }
    /// A running host can still hold the configuration from before its first MCP connection.
    fn idle_state(
        &self,
        host: &Host,
        pending: bool,
        changed: &Value,
    ) -> (&'static str, &'static str) {
        let process = if pending {
            self.processes
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .state(&self.context, trae::process_id(host))
        } else {
            processes::State::Unknown
        };
        let old_process = matches!(process, processes::State::Running(started)
            if started > 0 && changed["at_ms"].as_u64().is_some_and(|at| started.saturating_mul(1000) <= at));
        if old_process {
            ("required", "host_running")
        } else if pending && !matches!(process, processes::State::Stopped) {
            ("unverified", "unverified")
        } else {
            ("idle", "no_connection")
        }
    }
    /// A new connection proves the current entry loaded; keep that proof once it closes.
    pub(crate) fn remember_reloads(&mut self) {
        let mut settings = self.settings.clone();
        let mut proved = false;
        for host in self.registry() {
            let Some(token) = self.reload_token(&host) else {
                continue;
            };
            let record = &self.settings[host.id];
            if record["reload_changed"]["v"] != RELOAD_FORMAT
                || record["reload_changed"]["token"] != token.as_str()
                || record["reload_confirmed"]["token"] == token.as_str()
                || self.reload_state(&host, &token)["state"] != "current"
            {
                continue;
            }
            settings[host.id]["reload_confirmed"] =
                json!({"v":RELOAD_FORMAT,"token":token,"at":timestamp()});
            proved = true;
        }
        if proved && roots::save(&self.context, &settings).is_ok() {
            self.settings = settings;
        }
    }
    fn connection_root(&self, host: &Host) -> Option<PathBuf> {
        let entry = host.read().ok()?.entry?;
        let explicit = entry["env"]["AGENTNAVE_DATA_DIR"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| self.context.env.get("AGENTNAVE_DATA_DIR").cloned());
        let root = explicit
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.context.home.join(".agentnave"));
        (root.is_absolute() && !root.is_symlink() && !root.join("connections").is_symlink())
            .then_some(root)
    }
    pub(super) fn note_reload_change(&self, host: &Host, before: &Value, settings: &mut Value) {
        let current = self.reload_token(host);
        if let Some(token) = current {
            let observed = &self.settings[host.id]["reload_observed"];
            let changed = if observed["v"] == RELOAD_FORMAT && observed["token"].is_string() {
                observed["token"] != token
            } else {
                *before != token
            };
            if !settings[host.id].is_object() {
                settings[host.id] = json!({});
            }
            settings[host.id]["reload_observed"] = json!({"v":RELOAD_FORMAT,"token":token});
            if changed {
                self.processes
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .invalidate();
                // A proof belongs to the entry it saw; returning to that entry later needs a new one.
                if settings[host.id]["reload_confirmed"]["token"] != token.as_str()
                    && let Some(record) = settings[host.id].as_object_mut()
                {
                    record.remove("reload_confirmed");
                }
                settings[host.id]["reload_changed"] = json!({"v":RELOAD_FORMAT,"token":token,"at_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64});
            }
        }
    }
}

impl Receipts {
    /// With no connection running, one that started after the change and closed after loading
    /// the current install's tools still proves the change loaded.
    fn proves_reload(&self, changed_at: Option<u64>, install: &Install, now: SystemTime) -> bool {
        let now_ms = now
            .duration_since(UNIX_EPOCH)
            .map_or(0, |at| at.as_millis() as u64);
        let (Some(at), Some(installed)) = (changed_at, install.version.as_deref()) else {
            return false;
        };
        !self.lingering_before_change
            && self.ended.iter().any(|c| {
                let (Some(start), Some(end)) = (c["started_ms"].as_u64(), c["ended_ms"].as_u64())
                else {
                    return false;
                };
                start > at
                    && start <= end
                    && end <= now_ms
                    && now_ms - end <= ENDED_SECONDS * 1000
                    && valid_hash(&c["runtime_tools_hash"])
                    && c["runtime_tools_hash"] == c["host_tools_hash"]
                    && c["runtime_version"].as_str() == Some(installed)
            })
    }
}

/// A private, size-limited receipt of this host's relays, with its age in seconds.
fn read_receipt(
    entry: &fs::DirEntry,
    now: SystemTime,
    boss: &str,
) -> Option<(u64, SystemTime, Value)> {
    let path = entry.path();
    if path.is_symlink() || path.extension().is_none_or(|ext| ext != "json") {
        return None;
    }
    let meta = entry.metadata().ok()?;
    let modified = meta.modified().ok()?;
    let age = now.duration_since(modified).ok()?.as_secs();
    if meta.len() > 16_384 || age > ENDED_SECONDS {
        return None;
    }
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (value["version"] == 1 && value["host"] == boss && value["dispatched"] != true)
        .then_some((age, modified, value))
}

fn receipt_pid(path: &Path) -> Option<u32> {
    let (pid, identity) = path.file_stem()?.to_str()?.split_once('-')?;
    if pid.is_empty()
        || !pid.bytes().all(|byte| byte.is_ascii_digit())
        || identity.len() != 32
        || !identity.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    pid.parse::<u32>().ok().filter(|pid| *pid > 0)
}

fn valid_hash(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Live connections that all run the delivered tools clear a reload when each one started after
/// the change, so each relay runs the current entry (ADR-0022).
fn live_state(
    connections: &[Value],
    install: &Install,
    reloaded: bool,
    base: (&'static str, &'static str),
) -> (&'static str, &'static str) {
    if connections.iter().any(|c| stale(c, install)) {
        return ("required", "stale");
    }
    if connections.iter().any(mismatched_tools) {
        return ("required", "tools");
    }
    let delivered = connections.iter().all(|c| {
        valid_hash(&c["runtime_tools_hash"]) && c["runtime_tools_hash"] == c["host_tools_hash"]
    });
    if !delivered || !reloaded {
        return base;
    }
    if install.version.is_none() {
        ("unknown", "connection")
    } else {
        ("current", "connection")
    }
}

fn stale(receipt: &Value, install: &Install) -> bool {
    install
        .version
        .as_deref()
        .is_some_and(|v| receipt["runtime_version"].as_str() != Some(v))
}

fn mismatched_tools(receipt: &Value) -> bool {
    valid_hash(&receipt["runtime_tools_hash"])
        && valid_hash(&receipt["host_tools_hash"])
        && receipt["runtime_tools_hash"] != receipt["host_tools_hash"]
}
