use super::*;

impl Hosts {
    pub fn refresh(&mut self) -> Value {
        let (shell, dirs) = probe_tools(&self.context);
        self.shell = Some(shell);
        *self.tools.lock().unwrap_or_else(|p| p.into_inner()) = Some(dirs);
        self.refresh_probed()
    }
    pub(crate) fn refresh_probed(&mut self) -> Value {
        self.codex_homes = self.other_codex_homes();
        self.remember_reloads();
        let mut settings = self.settings.clone();
        for host in self.registry() {
            let state = self.state(&host);
            if state["status"] != "absent" && settings[host.id].get("first_seen").is_none() {
                if !settings[host.id].is_object() {
                    settings[host.id] = json!({});
                }
                settings[host.id]["first_seen"] = json!(&self.detected_at);
            }
            if !self.detect(&host).0
                || !host.root.confirmed
                || self.settings_error.is_some()
                || (self.shell.as_ref().is_some_and(Result::is_err)
                    && !self.manual_roots.contains(host.id))
            {
                continue;
            }
            // Preflight every user layer before changing any of them. Project layers stay read-only.
            let layers = host
                .user_layers
                .iter()
                .map(|path| host.read_at(path).map(|document| (path, document.entry)))
                .collect::<Result<Vec<_>, _>>();
            let layers = match layers {
                Ok(layers) => layers,
                Err(error) => {
                    let (code, text) = error_parts(&error, "parse_failed");
                    self.record(host.id, Err(format!("{code}:{text}")), "");
                    continue;
                }
            };
            let signature = serde_json::to_string(&(host.id, &layers, "takeover")).unwrap();
            if self.failed.get(host.id) == Some(&signature) {
                continue;
            }
            let managed_before = layers.iter().any(|(_, entry)| {
                entry
                    .as_ref()
                    .is_some_and(|entry| managed_entry(&host, entry))
            });
            let result = self.refresh_layers(&host, &layers, managed_before);
            match result {
                Ok((changed, managed_entry)) => match self.ensure_skill(&host, managed_entry) {
                    Ok(skill_changed) if changed || skill_changed => {
                        let text = format!("已接管条目或更新 Skill{}", trust_hint(host.boss));
                        self.record(host.id, Ok(()), &text)
                    }
                    Ok(_) => {
                        self.failed.remove(host.id);
                        self.failures.remove(host.id);
                    }
                    Err(error) => {
                        // Retry Skill failures on every refresh; unlike a config conflict, a transient
                        // filesystem problem must not disable automatic repair for this host.
                        self.failed.remove(host.id);
                        self.record(host.id, Err(error), "");
                    }
                },
                Err(error) => {
                    self.failed.insert(host.id.into(), signature);
                    self.record(host.id, Err(error), "");
                }
            }
            self.note_reload_change(&host, &state["reload"]["token"], &mut settings);
        }
        if settings != self.settings && self.settings_error.is_none() {
            match roots::save(&self.context, &settings) {
                Ok(()) => self.settings = settings,
                Err(e) => self.settings_error = Some(e),
            }
        }
        self.cached = self.snapshot();
        self.prune_service_versions();
        self.cached.clone()
    }

    fn refresh_layers(
        &self,
        host: &Host,
        layers: &[(&PathBuf, Option<Value>)],
        managed_before: bool,
    ) -> Result<(bool, bool), String> {
        let mut changed = false;
        for (path, _) in layers {
            let mut layer = host.clone();
            layer.config = (*path).clone();
            changed |= transaction::edit(
                transaction::Target {
                    path,
                    home: &self.context.home,
                    app: &self.context.app,
                    id: host.id,
                    format: host.format,
                    key: host.key,
                },
                |latest| {
                    let Some(desired) = latest.as_mut() else {
                        return Ok(());
                    };
                    set_host_env(desired, host, "takeover_failed")?;
                    // A missing launcher is repointed, and so is an older version of our
                    // own: after a versioned install the old directory still exists, so
                    // "present" alone would leave every host on the previous service.
                    if let Some(path) = self.launcher()
                        && let Some(command) = layer.command(desired)
                        && entry_kind(command) == "agentnave"
                        && (self.command_state(&layer, desired) == "missing"
                            || (own_launcher(command) && !same_path(command, &path)))
                    {
                        layer.set_command(desired, &path);
                    }
                    Ok(())
                },
            )?;
        }
        // Require an AgentNave entry both before and after the transaction; the
        // post-transaction check prevents a concurrently removed entry from being revived.
        Ok((changed, managed_before && self.has_managed_entry(host)?))
    }
}
