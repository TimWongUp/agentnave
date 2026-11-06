use super::*;

impl Hosts {
    pub fn action(&mut self, operation: &str, payload: &Value) -> Result<Value, String> {
        let id = payload["id"]
            .as_str()
            .filter(|id| self.registry().iter().any(|host| host.id == *id))
            .ok_or("未知宿主 id")?;
        if operation == "root" {
            return self.set_root(id, payload);
        }
        let host = self.registry().into_iter().find(|h| h.id == id).unwrap();
        #[cfg(unix)]
        if operation == "reload-copy" {
            return self.reload_copy(&host, payload);
        }
        if operation == "skill" {
            return self.change_skill(id, &host, payload);
        }
        if operation == "confirm-reload" {
            return self.confirm_reload(id, &host, payload);
        }
        if !matches!(operation, "connect" | "remove") {
            return Err("未知宿主操作".into());
        }
        let before_reload = self.state(&host)["reload"]["token"].clone();
        let result: Result<String, String> = (|| {
            if operation == "remove" {
                return self.remove_host(id, &host);
            }
            self.connect_host(id, &host, operation)
        })();
        let ok = result.is_ok();
        if operation == "connect" {
            let mut settings = self.settings.clone();
            self.note_reload_change(&host, &before_reload, &mut settings);
            if settings != self.settings && self.settings_error.is_none() {
                roots::save(&self.context, &settings)?;
                self.settings = settings;
            }
        }
        let mut text = result
            .as_ref()
            .cloned()
            .unwrap_or_else(|e| message(e).into());
        if ok
            && operation == "connect"
            && self
                .reload_token(&host)
                .is_some_and(|token| json!(token) != before_reload)
        {
            let reload = self.state(&host)["reload"].clone();
            match reload["state"].as_str() {
                Some("required") => text.push_str(&format!("；{}后生效", reload_hint(id))),
                Some("unverified") => {
                    text.push_str(&format!("；待确认生效，{}后再使用", reload_hint(id)))
                }
                Some("idle") => text.push_str("；下次启动宿主时生效"),
                _ => {}
            }
        }
        self.failed.remove(id);
        self.record(id, result.map(|_| ()), &text);
        if ok {
            self.manual_roots.insert(id.into());
        }
        let current = self.registry().into_iter().find(|h| h.id == id).unwrap();
        self.cached = self.snapshot();
        Ok(json!({"host":self.state(&current),"ok":ok,"message":text}))
    }
    pub(super) fn record(&mut self, id: &str, result: Result<(), String>, success: &str) {
        let ok = result.is_ok();
        let text = result.err().unwrap_or_else(|| success.into());
        if ok {
            self.failures.remove(id);
        } else {
            self.failures
                .insert(id.into(), error_attention(&text, "takeover_failed"));
        }
        self.actions.insert(
            id.into(),
            json!({"ok":ok,"message":message(&text),"at":timestamp()}),
        );
    }
    pub(super) fn write(
        &self,
        host: &Host,
        before: &Document,
        desired: Option<&Value>,
    ) -> Result<bool, String> {
        transaction::edit(
            transaction::Target {
                path: &host.config,
                home: &self.context.home,
                app: &self.context.app,
                id: host.id,
                format: host.format,
                key: host.key,
            },
            |latest| {
                if let Some(desired) = desired {
                    if latest.is_none() && before.entry.is_some() {
                        return Err("conflict:条目已被其他进程移除，请刷新".into());
                    }
                    if latest.is_none() {
                        *latest = Some(json!({}));
                    }
                    patch(latest.as_mut().unwrap(), before.entry.as_ref(), desired);
                } else {
                    *latest = None;
                }
                Ok(())
            },
        )
    }
    /// Installs the manager Skill and those required beside it, and each optional Skill the user
    /// switched on; App copies of all of them are updated to this release.
    pub(super) fn ensure_skill(&self, host: &Host, install_missing: bool) -> Result<bool, String> {
        let Some(path) = &host.skill else {
            return Ok(false);
        };
        let mut changed = self.ensure_one(host, path, skill::MANAGER, install_missing)?;
        // A host connected before a required Skill existed has the manager and lacks that Skill.
        let connected = skill::state(path, &self.context.home, skill::MANAGER)["state"] == "app";
        for (name, embedded) in skill::REQUIRED {
            if let Some(path) = host.optional(name) {
                changed |= self.ensure_one(host, &path, embedded, install_missing || connected)?;
            }
        }
        for (name, embedded) in skill::OPTIONAL {
            if let Some(path) = host.optional(name) {
                let wanted = install_missing && self.skill_wanted(host.id, name);
                changed |= self.ensure_one(host, &path, embedded, wanted)?;
            }
        }
        Ok(changed)
    }
    pub(super) fn ensure_one(
        &self,
        host: &Host,
        path: &Path,
        embedded: skill::Files,
        install_missing: bool,
    ) -> Result<bool, String> {
        let state = skill::state(path, &self.context.home, embedded);
        // Another copy is the App's to replace, like a missing one.
        let missing = state["state"] == "missing" || state["state"] == "external";
        // Only an older copy is replaced. An App installed over a newer one must not push the
        // hosts back to its own Skill while the service stays on the newer version.
        let outdated = state["state"] == "app"
            && crate::update::parse(state["version"].as_str().unwrap_or_default())
                < crate::update::parse(VERSION);
        if (missing && install_missing) || outdated {
            skill::install(
                path,
                &self.context.app,
                host.id,
                &self.context.home,
                embedded,
            )?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub(super) fn skill_wanted(&self, id: &str, name: &str) -> bool {
        self.settings[id]["skills"][name] == true
    }
    /// Hosts can share one folder (CODEBUDDY_CONFIG_DIR); the copy stays while another wants it.
    pub(super) fn shared_wanted(&self, host: &Host, name: &str, path: &Path) -> bool {
        self.registry().iter().any(|other| {
            other.id != host.id
                && other.optional(name).as_deref() == Some(path)
                && self.skill_wanted(other.id, name)
        })
    }
    /// Switches one optional Skill: on installs it now if the host is connected (otherwise on the
    /// next connect), off removes only the App's own copy.
    pub(super) fn set_optional_skill(
        &mut self,
        host: &Host,
        name: &str,
        enabled: bool,
    ) -> Result<String, String> {
        let (name, embedded) = *skill::OPTIONAL
            .iter()
            .find(|(known, _)| *known == name)
            .ok_or("未知 Skill")?;
        if self.settings_error.is_some() {
            return Err("请先修复 hosts.json".into());
        }
        let path = host.optional(name).ok_or("此宿主不支持安装 Skill")?;
        let shared = !enabled && self.shared_wanted(host, name, &path);
        // Off removes before recording, so a failed removal leaves the switch on.
        if !enabled && !shared {
            skill::remove(
                &path,
                &self.context.app,
                host.id,
                &self.context.home,
                embedded,
            )?;
        }
        let mut settings = self.settings.clone();
        if !settings[host.id].is_object() {
            settings[host.id] = json!({});
        }
        if !settings[host.id]["skills"].is_object() {
            settings[host.id]["skills"] = json!({});
        }
        settings[host.id]["skills"][name] = json!(enabled);
        roots::save(&self.context, &settings)?;
        self.settings = settings;
        if shared {
            return Ok("已停用；另一宿主共用此目录且仍启用，副本保留".into());
        }
        if !enabled {
            return Ok("已停用并移除".into());
        }
        if !self.has_managed_entry(host)? {
            return Ok("已启用；接入此宿主时自动安装".into());
        }
        self.ensure_one(host, &path, embedded, true)?;
        Ok("已安装".into())
    }
    pub(super) fn has_managed_entry(&self, host: &Host) -> Result<bool, String> {
        for path in &host.user_layers {
            if host
                .read_at(path)?
                .entry
                .as_ref()
                .is_some_and(|entry| managed_entry(host, entry))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn set_root(&mut self, id: &str, payload: &Value) -> Result<Value, String> {
        let path = payload.get("path").ok_or("缺少 path")?;
        if !path.is_null()
            && !path.as_str().is_some_and(|p| {
                Path::new(p).is_absolute()
                    && !Path::new(p)
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
            })
        {
            return Err("根目录必须为绝对路径或 null".into());
        }
        if self.settings_error.is_some() {
            return Err("请先修复 hosts.json".into());
        }
        // Compare against the entry before the move, so an equivalent root records no change.
        let before = self
            .registry()
            .into_iter()
            .find(|h| h.id == id)
            .and_then(|h| self.reload_token(&h))
            .map_or(Value::Null, Value::from);
        let mut settings = self.settings.clone();
        if !settings[id].is_object() {
            settings[id] = json!({});
        }
        if path.is_null() {
            settings[id].as_object_mut().unwrap().remove("root");
        } else {
            settings[id]["root"] = path.clone();
        }
        roots::save(&self.context, &settings)?;
        self.settings = settings;
        self.codex_homes = self.other_codex_homes();
        self.manual_roots.remove(id);
        if !path.is_null() {
            self.manual_roots.insert(id.into());
        }
        self.failed.remove(id);
        self.failures.remove(id);
        self.record(id, Ok(()), "已更新配置根目录");
        let host = self.registry().into_iter().find(|h| h.id == id).unwrap();
        let mut settings = self.settings.clone();
        self.note_reload_change(&host, &before, &mut settings);
        if settings != self.settings {
            roots::save(&self.context, &settings)?;
            self.settings = settings;
        }
        self.cached = self.snapshot();
        Ok(json!({"host":self.state(&host),"ok":true,"message":"已更新配置根目录"}))
    }
    fn change_skill(&mut self, id: &str, host: &Host, payload: &Value) -> Result<Value, String> {
        let name = payload["skill"].as_str().ok_or("缺少 skill")?;
        let enabled = payload["enabled"].as_bool().ok_or("缺少 enabled")?;
        let result = self.set_optional_skill(host, name, enabled);
        let ok = result.is_ok();
        let text = result.unwrap_or_else(|e| message(&e).into());
        self.actions
            .insert(id.into(), json!({"ok":ok,"message":text,"at":timestamp()}));
        self.cached = self.snapshot();
        Ok(json!({"host":self.state(host),"ok":ok,"message":text}))
    }
    fn confirm_reload(&mut self, id: &str, host: &Host, payload: &Value) -> Result<Value, String> {
        if self.settings_error.is_some() {
            return Err("请先修复 hosts.json".into());
        }
        let current = self.state(host);
        if current["status"] != "connected" {
            return Err("请先修复宿主接入，再确认重启".into());
        }
        let token = current["reload"]["token"]
            .as_str()
            .ok_or("无法确认此宿主，请先修复接入并刷新检测")?;
        if payload["token"].as_str() != Some(token) {
            return Err("服务或接入配置已变化，请刷新并重新启动宿主后再确认".into());
        }
        let mut settings = self.settings.clone();
        if !settings[id].is_object() {
            settings[id] = json!({});
        }
        settings[id]["reload_confirmed"] =
            json!({"v":RELOAD_FORMAT,"token":token,"at":timestamp()});
        roots::save(&self.context, &settings)?;
        self.settings = settings;
        self.cached = self.snapshot();
        Ok(
            json!({"host":self.state(host),"ok":true,"message":"已记录你已重启或重连；未自动验证宿主接口"}),
        )
    }
    fn remove_host(&mut self, id: &str, host: &Host) -> Result<String, String> {
        // Preflight all layers; each subsequent replacement is independently checked.
        for path in &host.user_layers {
            safe_path(path, &self.context.home)?;
            host.read_at(path)
                .map_err(|e| format!("parse_failed:{e}"))?;
        }
        let mut removed = Vec::new();
        // Clear before the first mutation: a later partial removal may fail.
        if self.settings[id].get("reload_confirmed").is_some() {
            let mut settings = self.settings.clone();
            settings[id]
                .as_object_mut()
                .unwrap()
                .remove("reload_confirmed");
            roots::save(&self.context, &settings)?;
            self.settings = settings;
        }
        for path in &host.user_layers {
            let mut layer = host.clone();
            layer.config = path.clone();
            let before = layer.read().map_err(|e| format!("parse_failed:{e}"))?;
            if self.write(&layer, &before, None)? {
                removed.push(path.display().to_string());
            }
        }
        if let Some(path) = &host.skill {
            skill::remove(
                path,
                &self.context.app,
                id,
                &self.context.home,
                skill::MANAGER,
            )?;
        }
        for (name, embedded) in skill::REQUIRED {
            if let Some(path) = host.optional(name) {
                skill::remove(&path, &self.context.app, id, &self.context.home, embedded)?;
            }
        }
        for (name, embedded) in skill::OPTIONAL {
            if let Some(path) = host.optional(name)
                && !self.shared_wanted(host, name, &path)
            {
                skill::remove(&path, &self.context.app, id, &self.context.home, embedded)?;
            }
        }
        Ok(if removed.is_empty() {
            "没有用户级条目需要移除".into()
        } else {
            format!("已移除：{}", removed.join("、"))
        })
    }
    fn connect_host(&mut self, id: &str, host: &Host, operation: &str) -> Result<String, String> {
        if !self.detect(host).0 {
            return Err("write_failed:宿主未安装".into());
        }
        let before = host.read().map_err(|e| format!("parse_failed:{e}"))?;
        crate::update::ensure_active_launcher(&self.context.app)
            .map_err(|error| format!("write_failed:{error}"))?;
        let path = self
            .launcher()
            .ok_or("write_failed:请先安装或更新本机服务")?;
        let mut desired = before.entry.clone();
        match operation {
            "connect" => {
                if let Some(value) = desired.as_mut() {
                    host.set_command(value, &path);
                    set_host_env(value, host, "write_failed")?;
                } else {
                    if id == "zcode"
                        && let Some(fallback) = zcode_fallback(&self.context.home, &before)
                    {
                        return Err(format!(
                            "write_failed:ZCode 只在 ~/.zcode/cli/config.json 没有 MCP 服务时读取 {}，接入后其中的服务将不再加载；请先把它们移到 ~/.zcode/cli/config.json 的 mcp.servers 再接入",
                            fallback.display()
                        ));
                    }
                    desired = Some(if id == "deepseek" {
                        // dsh cuts a tool call off at 60 s; `wait` blocks for up to 300 s.
                        json!({"serverName":"agentnave","transport":"stdio","command":path,"env":host_env(host),"toolCallTimeoutMs":310_000})
                    } else if id == "zcode" {
                        // ZCode stops a tool call after 30 s by default; `wait` blocks up to 300 s.
                        json!({"type":"stdio","command":path,"args":[],"env":host_env(host),"timeoutMs":310000})
                    } else if id == "hermes" {
                        json!({"command":path,"args":[],"env":host_env(host),"enabled":true})
                    } else {
                        json!({"command":path,"args":[],"env":host_env(host)})
                    });
                }
            }
            _ => return Err("未知宿主操作".into()),
        }
        self.write(host, &before, desired.as_ref())?;
        // The MCP entry is already connected even if the following Skill write fails.
        self.manual_roots.insert(id.into());
        let skill_changed = operation == "connect" && self.ensure_skill(host, true)?;
        let done = if skill_changed {
            "已接入并安装或更新 Skill"
        } else {
            "已接入；Skill 已是最新"
        };
        Ok(format!("{done}{}", trust_hint(id)))
    }
}
