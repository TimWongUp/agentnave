//! Connection faults a relay recorded when its server died under an open host (ADR-0088).
use super::*;

/// The relay clears records older than this when it writes one; the tray ignores them too.
const FAULT_SECONDS: u64 = 86_400;

impl Hosts {
    /// Fresh records of this host's relays, newest first with the path each came from.
    fn connection_faults(&self, host: &Host) -> Vec<(PathBuf, Value)> {
        let Some(dir) = self
            .connection_root(host)
            .map(|root| root.join("connection-faults"))
            .filter(|dir| !dir.is_symlink())
        else {
            return Vec::new();
        };
        let (now, sole) = (SystemTime::now(), self.sole_row(host));
        let mut faults: Vec<_> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let (_, _, value) = reload::read_receipt(&entry, now, host, sole, FAULT_SECONDS)?;
                value["fault_ms"].as_u64()?;
                Some((entry.path(), value))
            })
            .collect();
        faults.sort_by_key(|(_, value)| std::cmp::Reverse(value["fault_ms"].as_u64()));
        faults
    }
    pub(super) fn fault_state(&self, host: &Host) -> Value {
        let faults = self.connection_faults(host);
        match faults.first() {
            Some((_, latest)) => json!({"count":faults.len(),"at_ms":latest["fault_ms"]}),
            None => Value::Null,
        }
    }
    /// The user has seen the notice; only records this host's relays wrote are removed.
    pub(super) fn dismiss_faults(&mut self, host: &Host) -> Result<Value, String> {
        for (path, _) in self.connection_faults(host) {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("无法清除连接中断提示：{error}")),
            }
        }
        self.cached = self.snapshot();
        Ok(json!({"host":self.state(host),"ok":true,"message":"已清除连接中断提示"}))
    }
}
