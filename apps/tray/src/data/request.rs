use super::*;

impl Reader {
    pub(crate) fn video_packs_request(
        &self,
        app: Option<&tauri::AppHandle>,
        method: &str,
        path: &str,
        action: Option<&str>,
        body: &[u8],
    ) -> Response {
        crate::video_packs::request(&self.root, app, method, path, action, body)
    }

    pub(crate) fn video_asset(&self, path: &str, range: Option<&str>) -> Response {
        path.strip_prefix("/art/video/")
            .and_then(|rest| crate::video_packs::asset(&self.root, rest, range))
            .unwrap_or_else(Response::not_found)
    }

    /// Writes require an explicit POST and action header; navigation cannot mutate files.
    pub fn request(&self, method: &str, path: &str, action: Option<&str>, body: &[u8]) -> Response {
        // Live protocol requests pass `Some(app)` for the native picker; headless callers reach here.
        if path == "/app/video-packs" || path.starts_with("/app/video-packs/") {
            return self.video_packs_request(None, method, path, action, body);
        }
        if path == "/app/voice" || path.starts_with("/app/voice/") {
            return crate::voice::request(&self.root, path, method, action, body);
        }
        if path == "/app/hosts" || path.starts_with("/app/hosts/") {
            let response =
                crate::hosts::request(&self.root, &self.hosts, method, path, action, body);
            if method == "POST" && matches!(path, "/app/hosts/root" | "/app/hosts/refresh") {
                let hosts = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(hosts) = hosts.as_ref() {
                    self.update_boss_homes(hosts);
                }
            }
            return response;
        }
        if path == "/app/skills" {
            return crate::hosts::skill_request(method);
        }
        if let Some(family) = path.strip_prefix("/app/characters/") {
            return crate::characters::request(&self.root, family, method, action, body);
        }
        if matches!(
            path,
            "/app/cli-updates" | "/app/cli-updates/update" | "/app/cli-install-commands"
        ) {
            return crate::detect::update::request(
                method,
                path,
                action,
                body,
                &crate::members::codex_source,
            );
        }
        if matches!(path, "/app/review" | "/app/permissions" | "/app/members") {
            return crate::members::request(&self.root, method, path, action, body);
        }
        if path == "/app/provider-health/clear" {
            return crate::provider_health::request(&self.root, method, action, body);
        }
        if path == "/app/storage" || path.starts_with("/app/storage/") {
            return crate::storage::request(&self.root, method, path, action);
        }
        if path == "/app/settings" {
            return crate::settings::request(&self.root, method, action, body);
        }
        if matches!(
            path,
            "/app/orchestration" | "/app/orchestration/host-cli" | "/app/rules" | "/app/rules/open"
        ) {
            return crate::hosts::orchestration_request(
                &self.root,
                &self.hosts,
                method,
                path,
                action,
                body,
            );
        }
        if matches!(path, "/app/decision-seats" | "/app/decision-key") {
            return crate::hosts::decision_request(&self.root, method, path, action, body);
        }
        if matches!(
            path,
            "/app/conversations/rename"
                | "/app/conversations/delete"
                | "/app/workspaces/handover"
                | "/app/workspaces/forget"
        ) {
            return crate::manage::request(&self.root, method, path, action, body);
        }
        if method != "GET" {
            return Response::error(405, "此路由只接受 GET");
        }
        self.respond(path)
    }
    /// This phase only runs the bounded shell probe, never reads host configuration.
    pub(crate) fn probe_tools(&self, context: &crate::hosts::Context) -> crate::hosts::Shell {
        let (shell, dirs) = crate::hosts::probe_tools(context);
        crate::proxy::cache_network(&dirs.network);
        *self.tools.lock().unwrap_or_else(|p| p.into_inner()) = Some(dirs);
        shell
    }
    #[cfg(test)]
    pub(crate) fn use_hosts(&self, hosts: crate::hosts::Hosts) {
        self.update_boss_homes(&hosts);
        *self.hosts.lock().unwrap() = Some(hosts);
    }
    pub fn configure_hosts(&self, context: crate::hosts::Context, shell: crate::hosts::Shell) {
        let mut guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        let hosts = crate::hosts::Hosts::from_probe(context, Some(shell), self.tools.clone());
        self.update_boss_homes(&hosts);
        *guard = Some(hosts);
    }
    pub fn prepare_members(&self) {
        // Probe before taking the lock: probing runs every CLI and would hold up the host page.
        let detected = crate::detect::detected(false, crate::members::codex_source);
        let guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(hosts) = guard.as_ref() {
            crate::members::prepare(&self.root, hosts.cached_snapshot(), &detected);
        }
        crate::members::mark_prepared();
    }
    pub(super) fn boss_homes(&self) -> boss_app::Homes {
        // A host refresh holds the lock for seconds; only then does the last snapshot stand in
        // for the registry, which can change on its own (Claude Desktop installed or removed).
        let free = match self.hosts.try_lock() {
            Ok(hosts) => Some(hosts),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        };
        if let Some(hosts) = free.as_ref().and_then(|hosts| hosts.as_ref()) {
            self.update_boss_homes(hosts);
        }
        self.boss_homes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    fn update_boss_homes(&self, hosts: &crate::hosts::Hosts) {
        let homes = hosts.boss_homes();
        *self.boss_homes.lock().unwrap_or_else(|p| p.into_inner()) = homes;
    }
    pub fn refresh_hosts(&self) {
        if let Some(hosts) = self
            .hosts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            hosts.refresh_probed();
            crate::hosts::sync_orchestration(&self.root, hosts, &[]);
            self.update_boss_homes(hosts);
        }
    }
    pub(crate) fn tool_dirs(&self) -> Option<crate::tool_dirs::ToolDirs> {
        self.tools.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    /// `GET /settings/members`, re-probing the CLIs when `refresh` is set.
    pub fn members(&self, refresh: bool) -> Response {
        Response::json(&crate::members::state(&self.root, refresh))
    }
}
