//! Onboarding is a read-only projection of service, configuration and saved sessions.
use crate::hosts::Hosts;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

pub fn state(hosts: &Hosts, root: &Path) -> Value {
    let snapshot = hosts.snapshot();
    let service = snapshot["service"]["installed"] == true;
    let host = snapshot["hosts"]
        .as_array()
        .is_some_and(|hosts| hosts.iter().any(|h| h["status"] == "connected"));
    let members = crate::members::read(root).ok().flatten().is_some_and(|v| {
        crate::members::validate(&v).is_ok()
            && v["clis"].as_object().is_some_and(|clis| {
                clis.values()
                    .any(|c| c["models"].as_object().is_some_and(|m| !m.is_empty()))
            })
    });
    let tried = fs::read_dir(root.join("conversations"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|item| {
            let path = item.path();
            if path.is_symlink() || !path.is_dir() {
                return false;
            }
            fs::read(path.join("state.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|v| v["manager_session"].as_str().is_some_and(|s| !s.is_empty()))
        });
    json!({"steps": [
        {"id": "service", "done": service, "detail": if service { "本机服务已安装" } else { "请先安装本机服务" }},
        {"id": "host", "done": host, "detail": if host { "已有宿主接入" } else { "接入一个已安装的宿主" }},
        {"id": "members", "done": members, "detail": if members { "已配置组员" } else { "在设置中配置组员" }},
        {"id": "try", "done": tried, "detail": if tried { "已有主控会话记录" } else { "在宿主中尝试一次 AgentNave" }}
    ], "dismissed": hosts.context.app.join("onboarding.dismissed").is_file()})
}

pub fn dismiss(hosts: &Hosts) -> Result<Value, String> {
    super::hosts::atomic(&hosts.context.app.join("onboarding.dismissed"), b"1")?;
    Ok(json!({"dismissed": true}))
}
