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

    pub(crate) fn accounts_request(
        &self,
        app: Option<&tauri::AppHandle>,
        method: &str,
        path: &str,
        action: Option<&str>,
        body: &[u8],
    ) -> Response {
        let idle = || self.tasks_idle();
        let ctx = crate::accounts::Context {
            root: &self.root,
            home: dirs::home_dir().unwrap_or_default(),
            hosts: &self.hosts,
            idle: &idle,
        };
        let response = crate::accounts::request(&ctx, app, method, path, action, body);
        if method == "POST" {
            // The account's folder joins or leaves the places chats are looked for.
            let hosts = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(hosts) = hosts.as_ref() {
                self.update_boss_homes(hosts);
            }
        }
        response
    }

    /// Whether no task or meeting runs, or why one does.
    fn tasks_idle(&self) -> Result<(), String> {
        let overview = self.overview();
        if overview.count("running") + overview.count("review") > 0 {
            return Err("还有任务在跑，结束之后才能删除账号目录".into());
        }
        if overview.running_uncertain() {
            return Err("无法确认运行状态，请恢复数据读取后重试".into());
        }
        Ok(())
    }

    pub(crate) fn projects_request(
        &self,
        app: Option<&tauri::AppHandle>,
        method: &str,
        path: &str,
        action: Option<&str>,
        body: &[u8],
    ) -> Response {
        match (method, path) {
            ("GET", "/app/projects") => Response::json(&json!({
                "projects": crate::projects::manual::values(&crate::projects::manual::list(&self.projects_dir))
            })),
            (_, "/app/projects/pick") => crate::response::action(method, action, || {
                let Some(app) = app else {
                    return Response::error(500, "无法打开文件夹选择器");
                };
                match crate::video_packs::picker::choose(app, true) {
                    Ok(cwd) => {
                        Response::json(&json!({"cwd": cwd.map(|cwd| cwd.display().to_string())}))
                    }
                    Err(error) => Response::error(500, &error),
                }
            }),
            (_, "/app/projects/add") => crate::response::post(method, action, body, |payload| {
                self.add_project(app, &payload)
            }),
            (_, "/app/projects/remove") => crate::response::post(method, action, body, |payload| {
                self.remove_project(&payload)
            }),
            _ => Response::not_found(),
        }
    }

    fn add_project(&self, app: Option<&tauri::AppHandle>, payload: &Value) -> Response {
        let cwd = payload["cwd"]
            .as_str()
            .or_else(|| payload["path"].as_str())
            .unwrap_or_default()
            .trim();
        let dir = Path::new(cwd);
        if !dir.is_absolute() || !dir.is_dir() {
            return Response::error(400, "文件夹不存在");
        }
        let project = crate::projects::classify_dir(dir);
        let database = crate::db::read(&self.root);
        let (_, snapshots) = self.scan_with(&database);
        let handovers = database
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .and_then(handovers)
            .unwrap_or_default()
            .into_iter()
            .collect();
        let mut cwds: Vec<String> = crate::stage::origin_cwds(&snapshots, &handovers)
            .into_values()
            .collect();
        if let Some(app) = app {
            use tauri::Manager;
            cwds.extend(app.state::<crate::terminal::Terminals>().cwds());
        }
        let existing = crate::projects::resolve_all(cwds);
        match crate::projects::manual::add(&self.projects_dir, project, &existing) {
            Ok((entry, added, projects)) => {
                self.routes
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove("/app/chats");
                Response::json(&json!({
                    "project": entry.value(),
                    "added": added,
                    "projects": crate::projects::manual::values(&projects)
                }))
            }
            Err((status, message)) => Response::error(status, &message),
        }
    }

    fn remove_project(&self, payload: &Value) -> Response {
        let key = payload["key"].as_str().unwrap_or_default().trim();
        match crate::projects::manual::remove(&self.projects_dir, key) {
            Ok((removed, projects)) => {
                self.routes
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove("/app/chats");
                Response::json(&json!({
                    "removed": removed,
                    "projects": crate::projects::manual::values(&projects)
                }))
            }
            Err((status, message)) => Response::error(status, &message),
        }
    }

    /// Writes require an explicit POST and action header; navigation cannot mutate files.
    pub fn request(&self, method: &str, path: &str, action: Option<&str>, body: &[u8]) -> Response {
        if self.uninstalling() && method != "GET" {
            return Response::error(409, "卸载已开始，请完成卸载或重新打开 App");
        }
        // Live protocol requests pass `Some(app)` for the native picker; headless callers reach here.
        if path == "/app/video-packs" || path.starts_with("/app/video-packs/") {
            return self.video_packs_request(None, method, path, action, body);
        }
        if path == "/app/projects" || path.starts_with("/app/projects/") {
            return self.projects_request(None, method, path, action, body);
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
        if path == "/app/accounts" || path.starts_with("/app/accounts/") {
            return self.accounts_request(None, method, path, action, body);
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
        if path == "/app/provider-health" {
            return if method == "GET" {
                // The names the page shows beside each account's cooldown.
                let names: serde_json::Map<String, Value> = crate::accounts::read(&self.root)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|account| (account.id, json!(account.label)))
                    .collect();
                Response::json(&json!({
                    "providers": crate::provider_health::active(&self.root),
                    "accounts": names,
                }))
            } else {
                Response::error(405, "此路由只接受 GET")
            };
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
                | "/app/workspaces/cancel-handover"
                | "/app/workspaces/forget"
        ) {
            return crate::manage::request(
                &self.root,
                &self.projects_dir,
                method,
                path,
                action,
                body,
            );
        }
        if matches!(path, "/app/isolated/inspect" | "/app/isolated/delete") {
            return crate::isolated::request(&self.root, method, path, action, body);
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
            if self.uninstalling() {
                return;
            }
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
