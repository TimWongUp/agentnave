//! Cheap projection: cached host roots, no probing and no writes on GET.
use crate::hosts::{Hosts, atomic};
use serde_json::{Value, json};
use std::{fs, path::Path};
pub fn state(hosts: &Hosts, root: &Path) -> Value {
    let snapshot = hosts.cached_snapshot();
    let service = hosts.service()["installed"] == true;
    let connected = snapshot["hosts"]
        .as_array()
        .map(|h| h.iter().filter(|h| h["status"] == "connected").count())
        .unwrap_or(0);
    let members =
        root.join("members.json").is_file() || root.join("profiles.json.migrated").is_file();
    let tried = fs::read_dir(root.join("conversations"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            if entry.path().is_symlink() {
                return false;
            }
            fs::read(entry.path().join("state.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .is_some_and(|v| v["manager_session"].as_str().is_some_and(|s| !s.is_empty()))
        });
    json!({"steps":[
        {"id":"service","done":service,"detail":if service{"工作室与核心入口均已安装"}else{"宿主要通过本机服务才能启动 AgentNave"}},
        {"id":"host","done":connected>0,"detail":if connected>0{format!("已接入 {connected} 个宿主")}else{"在设置中接入一个宿主".into()}},
        {"id":"members","done":members,"detail":if members{"已保存或迁移组员设置"}else{"选择可以作为组员的 CLI 与模型"}},
        {"id":"try","done":tried,"detail":if tried{"已有主控会话记录"}else{"在宿主里让 AgentNave 派一次任务"}}
    ],"dismissed":hosts.context.app.join("onboarding.dismissed").is_file()})
}
pub fn dismiss(hosts: &Hosts) -> Result<Value, String> {
    let path = hosts.context.app.join("onboarding.dismissed");
    crate::hosts::check_path(&path, &hosts.context.home)?;
    atomic(&path, b"1")?;
    Ok(json!({"dismissed":true}))
}
pub fn reopen(hosts: &Hosts) -> Result<Value, String> {
    let path = hosts.context.app.join("onboarding.dismissed");
    crate::hosts::check_path(&path, &hosts.context.home)?;
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法重新显示开始使用".into()),
    };
    Ok(json!({"dismissed":false}))
}
