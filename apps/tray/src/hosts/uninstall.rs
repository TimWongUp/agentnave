//! Uninstall uses the complete registry, including hosts whose CLI has already gone away.
use super::*;

impl Hosts {
    pub(crate) fn uninstall(&mut self) -> Vec<Value> {
        self.uninstalling = true;
        let mut failures = Vec::new();
        for host in self.registry() {
            if let Err(error) = self.remove_host(host.id, &host) {
                failures.push(json!({"host": host.label, "error": message(&error)}));
            }
        }
        failures.extend(self.remove_retired());
        if let Err(error) = hooks::remove_interactive(&self.context.home) {
            failures.push(json!({"host": "agy hook", "error": message(&error)}));
        }
        // Use an explicitly disabled document: malformed saved orchestration must not prevent
        // removing managed blocks, nor may a failed host retain its authorization block.
        for failure in self.sync_rules(&json!({"preset": "off"}), &[]) {
            failures.push(json!({"host": failure["path"], "error": failure["error"]}));
        }
        failures
    }
}
