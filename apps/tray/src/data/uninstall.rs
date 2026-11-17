use super::*;
use std::sync::atomic::Ordering;

impl Reader {
    pub(crate) fn uninstall_ready(&self) -> bool {
        self.hosts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    pub(crate) fn uninstall_hosts(&self) -> Result<Vec<Value>, String> {
        let mut hosts = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        let hosts = hosts.as_mut().ok_or("宿主接入尚未初始化")?;
        // Set while holding the same lock used by startup repair and host actions. Once a
        // removal has begun this process never automatically reconnects a partially removed host.
        self.uninstalling.store(true, Ordering::SeqCst);
        Ok(hosts.uninstall())
    }

    pub(crate) fn uninstalling(&self) -> bool {
        self.uninstalling.load(Ordering::SeqCst)
    }

    pub(crate) fn data_root(&self) -> &Path {
        &self.root
    }
}
