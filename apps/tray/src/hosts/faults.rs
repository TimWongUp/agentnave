//! Connection faults a relay recorded when its server died under an open host (ADR-0088).
//! A newer healthy connection of the same row answers a fault (ADR-0096).
use super::*;

/// The relay clears records older than this when it writes one; the tray ignores them too.
const FAULT_SECONDS: u64 = 86_400;

/// This row's fresh records: still open newest first, or answered by a newer connection.
#[derive(Default)]
struct Faults {
    open: Vec<(PathBuf, Value)>,
    recovered: Vec<PathBuf>,
}

impl Hosts {
    /// Fresh fault records under this row's data root, whoever wrote them.
    fn fault_records(&self, host: &Host) -> Vec<(PathBuf, Value)> {
        let Some(dir) = self
            .connection_root(host)
            .map(|root| root.join("connection-faults"))
            .filter(|dir| !dir.is_symlink())
        else {
            return Vec::new();
        };
        let now = SystemTime::now();
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let (_, _, value) = reload::read_record(&entry, now, FAULT_SECONDS)?;
                value["fault_ms"].as_u64()?;
                Some((entry.path(), value))
            })
            .collect()
    }
    /// Whether this row's entry names the row its relays write, as the App's takeover does.
    fn labelled(&self, host: &Host) -> bool {
        host.read()
            .ok()
            .and_then(|document| document.entry)
            .is_some_and(|entry| entry["env"]["AGENTNAVE_HOST_APP"].is_string())
    }
    fn faults(&self, host: &Host) -> Faults {
        // A labelled row's relays label their records, so an unlabelled one is not its own.
        let unambiguous = self.sole_row(host) && !self.labelled(host);
        let healthy = self.healthy_since(host);
        let mut faults = Faults::default();
        for (path, value) in self.fault_records(host) {
            if !reload::owns(&value, host, unambiguous) {
                continue;
            }
            let at = value["fault_ms"].as_u64().unwrap_or_default();
            if healthy.is_some_and(|started| started > at) {
                faults.recovered.push(path);
            } else {
                faults.open.push((path, value));
            }
        }
        faults
            .open
            .sort_by_key(|(_, value)| std::cmp::Reverse(value["fault_ms"].as_u64()));
        faults
    }
    pub(super) fn fault_state(&self, host: &Host) -> Value {
        let faults = self.faults(host);
        match faults.open.first() {
            Some((_, latest)) => json!({"count":faults.open.len(),"at_ms":latest["fault_ms"]}),
            None => Value::Null,
        }
    }
    /// The user has seen the notice; only records this host's relays wrote are removed.
    pub(super) fn dismiss_faults(&mut self, host: &Host) -> Result<Value, String> {
        let faults = self.faults(host);
        let paths = faults.open.into_iter().map(|(path, _)| path);
        for path in paths.chain(faults.recovered) {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("无法清除连接中断提示：{error}")),
            }
        }
        self.cached = self.snapshot();
        Ok(json!({"host":self.state(host),"ok":true,"message":"已清除连接中断提示"}))
    }
    /// On a background refresh, never a GET: remove records a newer connection answered, and
    /// unlabelled ones once every connected row of their boss labels its own.
    pub(super) fn prune_faults(&self) {
        let rows = self.registry();
        for host in &rows {
            let mut stale = self.faults(host).recovered;
            let connected = |row: &&Host| {
                row.boss == host.boss && row.read().ok().and_then(|d| d.entry).is_some()
            };
            if self.labelled(host) && rows.iter().filter(connected).all(|row| self.labelled(row)) {
                stale.extend(
                    self.fault_records(host)
                        .into_iter()
                        .filter(|(_, value)| {
                            value["host"] == host.boss && value["host_app"].is_null()
                        })
                        .map(|(path, _)| path),
                );
            }
            for path in stale {
                let _ = fs::remove_file(path);
            }
        }
    }
}
