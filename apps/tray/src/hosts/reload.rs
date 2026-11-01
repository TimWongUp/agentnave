//! Whether hosts still run an MCP entry from before its last change (ADR-0022).
use super::*;

/// Matches the relay's own report that an update has waited too long.
const STALE_RUNTIME_SECONDS: u64 = 600;
/// A receipt this old with no refresh belongs to a relay that exited without removing it.
const LINGER_SECONDS: u64 = 150;
/// Relays prune receipts, ended ones included, after an hour without a write.
const ENDED_SECONDS: u64 = 3600;

#[derive(Default)]
struct Receipts {
    live: Vec<Value>,
    lingering: bool,
    lingering_before_change: bool,
    ended: Vec<Value>,
}
struct Install {
    version: Option<String>,
    settled: bool,
    at_ms: Option<u128>,
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
        let install = self.install(now);
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
        json!({"state":state,"reason":reason,"token":token,"connections":receipts.live.len(),"confirmed_at":confirmed["at"]})
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
            let Some((age, value)) = read_receipt(&entry, now, host.boss) else {
                continue;
            };
            if !value["ended_ms"].is_null() {
                receipts.ended.push(value);
            } else if age <= 20 {
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
    fn install(&self, now: SystemTime) -> Install {
        let env = crate::update::active_tools(&self.context.app)
            .or_else(|| self.tool_dirs().map(|dirs| dirs.tools.join("agentnave")));
        let installed_at = env.as_deref().and_then(crate::update::installed_since);
        Install {
            version: env.as_deref().and_then(crate::update::installed_version),
            // A relay swaps to a new install within seconds once idle; much later it is stuck.
            settled: installed_at
                .and_then(|since| now.duration_since(since).ok())
                .is_some_and(|age| age.as_secs() > STALE_RUNTIME_SECONDS),
            at_ms: installed_at
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .map(|at| at.as_millis()),
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
                .state(&self.context, host.boss)
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
fn read_receipt(entry: &fs::DirEntry, now: SystemTime, boss: &str) -> Option<(u64, Value)> {
    let path = entry.path();
    if path.is_symlink() || path.extension().is_none_or(|ext| ext != "json") {
        return None;
    }
    let meta = entry.metadata().ok()?;
    let age = now.duration_since(meta.modified().ok()?).ok()?.as_secs();
    if meta.len() > 16_384 || age > ENDED_SECONDS {
        return None;
    }
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (value["version"] == 1 && value["host"] == boss).then_some((age, value))
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
    if connections.iter().any(|c| {
        valid_hash(&c["runtime_tools_hash"])
            && valid_hash(&c["host_tools_hash"])
            && c["runtime_tools_hash"] != c["host_tools_hash"]
    }) {
        return ("required", "tools");
    }
    let delivered = connections.iter().all(|c| {
        valid_hash(&c["runtime_tools_hash"]) && c["runtime_tools_hash"] == c["host_tools_hash"]
    });
    if !delivered || !reloaded {
        return base;
    }
    // Current relays read the version from every 2026-07-28 result; released ones before v0.35
    // only from server/discover. For those, a guess: one started before the install reports the
    // version once it swaps, so none means stuck.
    let before_install = |c: &Value| {
        install.at_ms.is_some_and(|at| {
            c["started_ms"]
                .as_u64()
                .is_some_and(|start| u128::from(start) < at)
        })
    };
    let unreported = |c: &Value| c["runtime_version"].as_str().is_none();
    let behind = |c: &&Value| {
        if unreported(c) {
            before_install(c)
        } else {
            c["runtime_version"].as_str() != install.version.as_deref()
        }
    };
    if install.version.is_none() {
        ("unknown", "connection")
    } else if connections.iter().any(|c| behind(&c)) {
        if install.settled {
            // Long after the install: busy all along, holding an unread result, or a relay
            // before v0.31.0 that cannot swap for 2026-07-28 hosts.
            if connections.iter().filter(behind).any(unreported) {
                ("required", "relay")
            } else {
                ("required", "stale")
            }
        } else {
            ("updating", "connection")
        }
    } else if connections.iter().any(unreported) {
        ("unknown", "connection") // running the install, but the host has not shown its version
    } else {
        ("current", "connection")
    }
}
