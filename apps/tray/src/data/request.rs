use super::*;

impl Reader {
    /// Writes require an explicit POST and action header; navigation cannot mutate files.
    pub fn request(&self, method: &str, path: &str, action: Option<&str>, body: &[u8]) -> Response {
        if path == "/app/voice" || path.starts_with("/app/voice/") {
            if method == "GET" {
                return crate::voice::get(&self.root, path);
            }
            return crate::voice::request(&self.root, path, method, action, body);
        }
        if path == "/app/hosts" || path.starts_with("/app/hosts/") {
            return self.host_request(method, path, action, body);
        }
        if path == "/app/skills" {
            if method != "GET" {
                return Response::error(405, "此路由只接受 GET");
            }
            return Response::json(&crate::hosts::skill_catalog());
        }
        if let Some(family) = path.strip_prefix("/app/characters/") {
            return crate::characters::request(&self.root, family, method, action, body);
        }
        if matches!(
            path,
            "/app/cli-updates" | "/app/cli-updates/update" | "/app/cli-install-commands"
        ) {
            return Self::cli_updates_request(method, path, action, body);
        }
        if path == "/app/review" {
            return post(method, action, body, |payload| {
                save(
                    &self.root,
                    &payload,
                    crate::members::validate_review,
                    crate::members::save_review,
                )
            });
        }
        if path == "/app/permissions" {
            return post(method, action, body, |payload| {
                save(
                    &self.root,
                    &payload,
                    crate::members::validate_permissions,
                    crate::members::save_permissions,
                )
            });
        }
        if path == "/app/members" {
            return post(method, action, body, |payload| {
                save(
                    &self.root,
                    &payload,
                    crate::members::validate,
                    crate::members::save,
                )
            });
        }
        if path == "/app/storage" || path.starts_with("/app/storage/") {
            return crate::storage::request(&self.root, method, path, action);
        }
        if matches!(
            path,
            "/app/settings" | "/app/orchestration" | "/app/rules" | "/app/rules/open"
        ) {
            return self.settings_request(method, path, action, body);
        }
        if path == "/app/conversations/rename" {
            return post(method, action, body, |payload| {
                let (Some(id), Some(title)) = (
                    payload["id"].as_str().filter(|id| valid_id(id)),
                    payload["title"].as_str(),
                ) else {
                    return Response::error(400, "需要有效的会话 ID 与标题");
                };
                Response::json(&crate::manage::rename(&self.root, id, title))
            });
        }
        if path == "/app/workspaces/handover" {
            return post(method, action, body, |payload| {
                let Some(group) = payload["group"].as_str().filter(|id| valid_id(id)) else {
                    return Response::error(400, "需要有效的工作区 ID");
                };
                Response::json(&crate::manage::handover(&self.root, group))
            });
        }
        if path == "/app/workspaces/forget" {
            return post(method, action, body, |payload| {
                let Some(group) = payload["group"].as_str().filter(|id| valid_id(id)) else {
                    return Response::error(400, "需要有效的工作区 ID");
                };
                Response::json(&crate::manage::forget(&self.root, group))
            });
        }
        if path == "/app/conversations/delete" {
            return post(method, action, body, |payload| {
                self.delete_conversations(&payload)
            });
        }
        if method != "GET" {
            return Response::error(405, "此路由只接受 GET");
        }
        self.respond(path)
    }
    fn cli_updates_request(
        method: &str,
        path: &str,
        action: Option<&str>,
        body: &[u8],
    ) -> Response {
        if path == "/app/cli-updates" || path == "/app/cli-install-commands" {
            if method != "GET" {
                return Response::error(405, "此路由只接受 GET");
            }
            return Response::json(&if path == "/app/cli-install-commands" {
                crate::detect::update::install_commands()
            } else {
                crate::detect::update::state()
            });
        }
        post(method, action, body, |payload| {
            let Some(provider) = payload["provider"]
                .as_str()
                .filter(|p| crate::members::PROVIDERS.contains(p))
            else {
                return Response::error(400, "需要有效的 CLI 名称");
            };
            match crate::detect::update::update(provider) {
                Ok(state) => Response::json(&state),
                Err(error) => Response::error(400, &error),
            }
        })
    }

    fn settings_request(
        &self,
        method: &str,
        path: &str,
        action: Option<&str>,
        body: &[u8],
    ) -> Response {
        if path == "/app/settings" {
            if method == "GET" {
                return Response::json(&crate::settings::state(&self.root));
            }
            if method != "POST" {
                return Response::error(405, "此路由只接受 GET 或 POST");
            }
            return with_payload(action, body, |payload| {
                save(
                    &self.root,
                    &payload,
                    crate::settings::validate,
                    crate::settings::save,
                )
            });
        }
        if path == "/app/orchestration" {
            if method == "GET" {
                return self.orchestration();
            }
            if method != "POST" {
                return Response::error(405, "此路由只接受 GET 或 POST");
            }
            return with_payload(action, body, |payload| self.save_orchestration(&payload));
        }
        if path == "/app/rules" || path == "/app/rules/open" {
            let home = dirs::home_dir().unwrap_or_default();
            let apps = self
                .hosts
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .map(crate::hosts::Hosts::rule_apps)
                .unwrap_or_default();
            if path == "/app/rules" {
                if method != "GET" {
                    return Response::error(405, "此路由只接受 GET");
                }
                return Response::json(&self.rule_files());
            }
            return post(method, action, body, |payload| {
                match crate::rules::open(&apps, &home, payload["path"].as_str().unwrap_or_default())
                {
                    Ok(result) => Response::json(&result),
                    Err(error) => Response::error(400, &error),
                }
            });
        }
        unreachable!("settings route checked by caller")
    }

    pub(super) fn orchestration(&self) -> Response {
        let (document, error) = match crate::members::read_orchestration(&self.root) {
            Ok(document) => (document, Value::Null),
            Err(error) => (json!({"version": 1, "preset": "off"}), json!(error)),
        };
        let rules = self.rule_files();
        Response::json(&json!({"document": document, "error": error,
            "presets": crate::rules::presets(), "files": rules["files"], "bosses": rules["bosses"]}))
    }
    pub(super) fn save_orchestration(&self, payload: &Value) -> Response {
        // The modes each row runs on are worked out here, never taken from the page.
        let mut document = payload.clone();
        if let Some(fields) = document.as_object_mut() {
            fields.remove("effective_apps");
        }
        if let Err(error) = crate::members::validate_orchestration(&document) {
            return Response::error(400, &error);
        }
        let guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        document["effective_apps"] = guard
            .as_ref()
            .map(|hosts| hosts.effective_apps(&document))
            .unwrap_or_else(|| json!({}));
        if let Err(error) = crate::members::save_orchestration(&self.root, &document) {
            return Response::error(500, &error);
        }
        let failures = guard
            .as_ref()
            .map(|hosts| self.sync_rules(hosts, &[]))
            .unwrap_or_default();
        drop(guard);
        let rules = self.rule_files();
        Response::json(&json!({"saved": true, "failures": failures,
            "files": rules["files"], "bosses": rules["bosses"]}))
    }
    pub(super) fn delete_conversations(&self, payload: &Value) -> Response {
        let Some(ids) = payload.get("ids").and_then(Value::as_array).filter(|ids| {
            !ids.is_empty() && ids.iter().all(|id| id.as_str().is_some_and(valid_id))
        }) else {
            return Response::error(400, "ids 必须为非空的有效会话 ID 数组");
        };
        let cutoff = match payload.get("updated_before") {
            None => None,
            Some(value) => match value.as_i64().filter(|cutoff| *cutoff > 0) {
                Some(cutoff) => Some(cutoff),
                None => return Response::error(400, "updated_before 必须是有效的毫秒时间"),
            },
        };
        let results: Vec<Value> = match cutoff {
            Some(cutoff) => crate::manage::cleanup(
                &self.root,
                &ids.iter()
                    .map(|id| id.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>(),
                cutoff,
            ),
            None => ids
                .iter()
                .map(|id| crate::manage::delete(&self.root, id.as_str().unwrap()))
                .collect(),
        };
        Response::json(&json!({"results": results}))
    }
    /// This phase only runs the bounded shell probe, never reads host configuration.
    pub(crate) fn probe_tools(&self, context: &crate::hosts::Context) -> crate::hosts::Shell {
        let (shell, dirs) = crate::hosts::probe_tools(context);
        *self.tools.lock().unwrap_or_else(|p| p.into_inner()) = Some(dirs);
        shell
    }
    #[cfg(test)]
    pub(crate) fn use_hosts(&self, hosts: crate::hosts::Hosts) {
        *self.hosts.lock().unwrap() = Some(hosts);
    }
    pub fn configure_hosts(&self, context: crate::hosts::Context, shell: crate::hosts::Shell) {
        let mut guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some(crate::hosts::Hosts::from_probe(
            context,
            Some(shell),
            self.tools.clone(),
        ));
    }
    pub fn prepare_members(&self) {
        // Probe before taking the lock: probing runs every CLI and would hold up the host page.
        let detected = crate::detect::detected(false);
        let guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(hosts) = guard.as_ref() {
            crate::members::prepare(&self.root, hosts.cached_snapshot(), &detected);
        }
        crate::members::mark_prepared();
    }
    pub(super) fn boss_homes(&self) -> boss_app::Homes {
        let hosts = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        hosts
            .as_ref()
            .map_or_else(boss_app::Homes::here, crate::hosts::Hosts::boss_homes)
    }
    pub fn refresh_hosts(&self) {
        if let Some(hosts) = self
            .hosts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            hosts.refresh_probed();
            self.sync_rules(hosts, &[]);
        }
    }
    /// Bring the orchestration block and the modes the server reads per row in line with the
    /// settings and the connected apps. A broken orchestration.json leaves the files alone; a
    /// preview build never writes them.
    pub(super) fn sync_rules(
        &self,
        hosts: &crate::hosts::Hosts,
        also: &[std::path::PathBuf],
    ) -> Vec<Value> {
        if crate::PREVIEW {
            return Vec::new();
        }
        let Ok(mut document) = crate::members::read_orchestration(&self.root) else {
            return Vec::new();
        };
        let mut failures = Vec::new();
        let effective = hosts.effective_apps(&document);
        if self.root.join("orchestration.json").is_file()
            && document.get("effective_apps") != Some(&effective)
        {
            document["effective_apps"] = effective;
            if let Err(error) = crate::members::save_orchestration(&self.root, &document) {
                failures
                    .push(json!({"path": self.root.join("orchestration.json"), "error": error}));
            }
        }
        failures.extend(hosts.sync_rules(&document, also));
        failures
    }
    /// The global rules files with whether each carries the current orchestration block.
    pub(super) fn rule_files(&self) -> Value {
        let home = dirs::home_dir().unwrap_or_default();
        let guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        let (installed, connected) = guard
            .as_ref()
            .map(|hosts| (hosts.rule_apps(), hosts.connected_rule_apps()))
            .unwrap_or_default();
        let document = crate::members::read_orchestration(&self.root)
            .unwrap_or_else(|_| json!({"preset": "off"}));
        crate::rules::list(&installed, &connected, &home, &document)
    }
    pub(crate) fn tool_dirs(&self) -> Option<crate::hosts::ToolDirs> {
        self.tools.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    pub(super) fn host_request(
        &self,
        method: &str,
        path: &str,
        action: Option<&str>,
        body: &[u8],
    ) -> Response {
        let read = path == "/app/hosts";
        let write = (cfg!(unix) && path == "/app/hosts/reload-copy")
            || matches!(
                path,
                "/app/hosts/refresh"
                    | "/app/hosts/connect"
                    | "/app/hosts/remove"
                    | "/app/hosts/root"
                    | "/app/hosts/confirm-reload"
                    | "/app/hosts/skill"
                    | "/app/hosts/reveal-backups"
            );
        if !read && !write {
            return Response::error(404, "未知路由");
        }
        if (read && method != "GET") || (write && method != "POST") {
            return Response::error(405, "此路由不接受该方法");
        }
        if write && action != Some("1") {
            return Response::error(403, "缺少操作请求头");
        }
        let mut guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        let Some(hosts) = guard.as_mut() else {
            return Response::error(503, "宿主接入尚未初始化");
        };
        match path {
            "/app/hosts" => {
                if !crate::PREVIEW {
                    hosts.remember_reloads();
                }
                Response::json(&hosts.snapshot())
            }
            "/app/hosts/refresh" => {
                let snapshot = hosts.refresh();
                self.sync_rules(hosts, &[]);
                Response::json(&snapshot)
            }
            "/app/hosts/reveal-backups" => match hosts.reveal_backups() {
                Ok(result) => Response::json(&result),
                Err(error) => Response::error(500, &error),
            },
            _ => {
                let Ok(payload) = serde_json::from_slice::<Value>(body) else {
                    return Response::error(400, "请求必须为 JSON");
                };
                let operation = path.rsplit('/').next().unwrap();
                // A moved host's old rules files still carry the block; clean them after the move.
                let before = if operation == "root" {
                    hosts.rule_paths()
                } else {
                    Vec::new()
                };
                // Judged before the connection counts itself as a boss that ran here.
                let used = operation == "connect"
                    && crate::members::used_before(&self.root, hosts.cached_snapshot());
                let result = hosts.action(operation, &payload);
                // Connecting, removing or moving a host changes which rules files carry the block.
                if matches!(operation, "connect" | "remove" | "root") {
                    self.sync_rules(hosts, &before);
                }
                self.host_action_response(operation, result, used)
            }
        }
    }

    fn host_action_response(
        &self,
        operation: &str,
        result: Result<Value, String>,
        used: bool,
    ) -> Response {
        let mut result = match result {
            Ok(result) => result,
            Err(error) => return Response::error(400, &error),
        };
        // The connection already took effect, so a failed member update is told, not an error.
        if operation == "connect"
            && result["ok"] == true
            && let Err(error) = crate::members::confirm_host(
                &self.root,
                result["host"]["id"].as_str().unwrap_or_default(),
                used,
            )
        {
            let message = result["message"].as_str().unwrap_or_default();
            result["message"] = json!(format!("{message}；员工名单未更新：{error}"));
        }
        Response::json(&result)
    }
    /// `GET /settings/members`, re-probing the CLIs when `refresh` is set.
    pub fn members(&self, refresh: bool) -> Response {
        Response::json(&crate::members::state(&self.root, refresh))
    }
}
