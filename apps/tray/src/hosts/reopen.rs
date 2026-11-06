//! User-facing guidance and the three native background-copy actions.
use super::*;

pub(super) fn instructions(host: &Host) -> Value {
    match host.boss {
        "claude" => json!({
            "daemon":"停止 Claude 后台服务，释放预热会话的旧连接；这会结束后台会话",
            "desktop":"完全退出 Claude 桌面端后重开，或结束并重开其中的 Code 会话",
            "cli":"退出当前 Claude Code 会话后重开，可用 claude --continue 接回"}),
        "codex" => json!({
            "daemon":"重启 Codex 后台服务；正在运行的后台或远程任务会中断",
            "app":"完全退出 Codex 或 ChatGPT 桌面端后重开",
            "ide":"在编辑器命令面板运行 Developer: Reload Window，或完全退出并重开编辑器",
            "cli":"退出当前 Codex 会话后重开，可用 codex resume 接回"}),
        "hermes" => json!({
            "gateway":"重启 Hermes 网关；消息平台上的对话会短暂中断并重新连接",
            "app":"完全退出 Hermes 桌面端后重开",
            "cli":"退出当前 Hermes 会话后重开"}),
        "workbuddy" => json!({"app":"完全退出 WorkBuddy 后重开","cli":"完全退出 WorkBuddy 后重开"}),
        "antigravity" => {
            json!({"app":"完全退出 Antigravity 后重开","cli":"退出当前 agy 会话后重开"})
        }
        "pi" => json!({"cli":"在 Pi 会话里输入 /reload，或重开 Pi"}),
        "codebuddy" => json!({"cli":"退出当前 CodeBuddy Code 会话后重开"}),
        "grok" => json!({"cli":"退出当前 Grok CLI 会话后重开"}),
        "deepseek" => {
            json!({"app":"完全退出 DeepSeek Harness 桌面端后重开","cli":"退出当前 dsh 会话后重开"})
        }
        "zcode" => {
            json!({"app":"完全退出 ZCode 桌面端后重开","cli":"在 ZCode 新建会话或退出后重开"})
        }
        _ => json!({}),
    }
}

#[cfg(unix)]
impl Hosts {
    pub(super) fn reload_copy(&mut self, host: &Host, payload: &Value) -> Result<Value, String> {
        let kind = payload["kind"].as_str().ok_or("缺少 kind")?;
        let (args, message): (&[&str], &str) = match (host.boss, kind) {
            ("claude", "daemon") => (&["daemon", "stop", "--any"], "已停止 Claude 后台服务"),
            ("codex", "daemon") => (
                &["app-server", "daemon", "restart"],
                "已重启 Codex 后台服务",
            ),
            ("hermes", "gateway") => (&["gateway", "restart"], "已重启 Hermes 网关"),
            _ => return Err("此宿主副本不支持一键重开，请按说明操作".into()),
        };
        let result = self.run_copy_action(host.binary, args);
        let ok = result.is_ok();
        let message = result.err().unwrap_or_else(|| message.into());
        self.processes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .invalidate();
        // No config, reload confirmation, Skill or rules writes, including in preview builds.
        Ok(json!({"ok":ok,"message":message,"host":self.state(host)}))
    }

    fn run_copy_action(&self, binary: &str, args: &[&str]) -> Result<(), String> {
        use crate::process::OutputError;
        let path = self.context.find(binary).ok_or("未找到此宿主的 CLI")?;
        let mut command = std::process::Command::new(path);
        command
            .args(args)
            .envs(&self.context.env)
            .current_dir(&self.context.home)
            .stdin(std::process::Stdio::null());
        let output = crate::process::output(command, std::time::Duration::from_secs(30)).map_err(
            |error| match error {
                OutputError::Timeout => "操作超时，请在宿主中检查后台服务状态".to_owned(),
                OutputError::Io(error) => format!("无法运行宿主命令：{error}"),
                OutputError::Reader => "无法读取宿主命令结果".to_owned(),
            },
        )?;
        if !output.status.success() {
            return Err("宿主未能完成操作，请在宿主中检查后台服务状态".into());
        }
        Ok(())
    }
}
