//! Known user/override/project configurations pin version directories, including custom rows.
use super::*;

impl Hosts {
    pub(super) fn prune_service_versions(&self) {
        let configs = self
            .registry()
            .iter()
            .try_fold(Vec::new(), |mut raw, host| {
                if !host.root.confirmed || self.settings_error.is_some() {
                    return None;
                }
                for layer in self.layers(host) {
                    let path = PathBuf::from(layer["path"].as_str()?);
                    safe_path(&path, &self.context.home).ok()?;
                    let document = host.read_at(&path).ok()?;
                    // Wrappers can hide an arbitrary old environment behind a command; don't guess.
                    if document.entry.as_ref().is_some_and(|entry| {
                        host.command(entry)
                            .is_none_or(|command| !own_launcher(command))
                    }) {
                        return None;
                    }
                    raw.push(document.raw);
                }
                Some(raw)
            });
        crate::update::prune(&self.context.app, configs);
    }
}
