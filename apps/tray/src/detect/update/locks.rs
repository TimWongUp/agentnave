use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) enum Environment {
    Native(PathBuf),
    Npm(PathBuf),
    Brew(PathBuf),
}

#[derive(Default)]
struct Running {
    providers: HashSet<String>,
    environments: HashSet<Environment>,
}

#[derive(Default)]
pub(super) struct Updates {
    running: Mutex<Running>,
    ready: Condvar,
}

pub(super) struct Claim<'a> {
    updates: &'a Updates,
    provider: String,
    environment: Option<Environment>,
}

impl Updates {
    pub(super) fn claim(&self, preview: bool, provider: &str) -> Result<Claim<'_>, String> {
        if preview {
            return Err("预览版不执行 CLI 更新".into());
        }
        let mut running = self.running.lock().unwrap_or_else(|p| p.into_inner());
        if !running.providers.insert(provider.to_owned()) {
            return Err("已有 CLI 正在更新此 Provider，请稍后重试".into());
        }
        Ok(Claim {
            updates: self,
            provider: provider.into(),
            environment: None,
        })
    }
}

impl Claim<'_> {
    /// Keep the environment until verification finishes, including while another CLI waits.
    pub(super) fn environment(&mut self, environment: &Environment) {
        let mut running = self
            .updates
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        while running.environments.contains(environment) {
            running = self
                .updates
                .ready
                .wait(running)
                .unwrap_or_else(|p| p.into_inner());
        }
        running.environments.insert(environment.clone());
        self.environment = Some(environment.clone());
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        let mut running = self
            .updates
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        running.providers.remove(&self.provider);
        if let Some(environment) = &self.environment {
            running.environments.remove(environment);
        }
        self.updates.ready.notify_all();
    }
}
