//! Cached, fixed-script login-shell discovery. Output is data, never executable input.
use super::{Context, atomic, transaction::safe_path};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(super) const VARIABLES: &[&str] = &[
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
    "CODEBUDDY_CONFIG_DIR",
    "PI_CODING_AGENT_DIR",
    "GROK_HOME",
    "HERMES_HOME",
    "UV_TOOL_DIR",
    "UV_TOOL_BIN_DIR",
    "XDG_BIN_HOME",
    "XDG_DATA_HOME",
];
pub(crate) type Shell = Result<BTreeMap<String, String>, String>;

pub(super) fn probe(context: &Context) -> Shell {
    let shell = context
        .env
        .get("SHELL")
        .filter(|p| Path::new(p).is_absolute())
        .ok_or("登录 shell 未知，根目录尚未确认")?;
    let script = VARIABLES
        .iter()
        .map(|name| format!("printf '%s\\0%s\\0' '{name}' \"${{{name}-}}\"\n"))
        .collect::<String>();
    let out = tempfile::tempfile().map_err(|_| "无法暂存 shell 探测结果")?;
    let mut command = Command::new(shell);
    command
        .args(["-l", "-c", &script])
        .env_clear()
        .envs(&context.env)
        .env("HOME", &context.home)
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(out.try_clone().map_err(|_| "无法探测 shell")?)
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|_| "无法启动登录 shell")?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return Err("登录 shell 探测失败".into()),
            Ok(None) if started.elapsed() < Duration::from_secs(3) => {
                thread::sleep(Duration::from_millis(15))
            }
            _ => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err("登录 shell 探测超时，根目录尚未确认".into());
            }
        }
    }
    use std::io::{Seek, SeekFrom};
    let mut out = out;
    out.seek(SeekFrom::Start(0))
        .map_err(|_| "无法读取探测结果")?;
    let mut bytes = Vec::new();
    out.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取探测结果")?;
    if bytes.len() > 65536 {
        return Err("登录 shell 输出过大".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "登录 shell 输出编码无效")?;
    let mut values = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    let parts: Vec<_> = text.split('\0').collect();
    for pair in parts.as_chunks::<2>().0 {
        if VARIABLES.contains(&pair[0]) {
            seen.insert(pair[0]);
        }
        if VARIABLES.contains(&pair[0])
            && Path::new(pair[1]).is_absolute()
            && !pair[1].contains('\n')
        {
            values.insert(pair[0].into(), pair[1].into());
        }
    }
    if seen.len() != VARIABLES.len() {
        return Err("登录 shell 输出不完整，根目录尚未确认".into());
    }
    Ok(values)
}

#[derive(Clone)]
pub(super) struct Root {
    pub path: PathBuf,
    pub source: &'static str,
    pub confirmed: bool,
}
pub(super) fn resolve(
    context: &Context,
    settings: &Value,
    shell: Option<&Shell>,
    id: &str,
    var: Option<&str>,
    default: PathBuf,
) -> Root {
    if let Some(path) = settings[id]["root"]
        .as_str()
        .filter(|p| Path::new(p).is_absolute())
    {
        return Root {
            path: path.into(),
            source: "user",
            confirmed: true,
        };
    }
    if let Some(path) = var
        .and_then(|v| context.env.get(v))
        .filter(|p| Path::new(p).is_absolute())
    {
        return Root {
            path: path.into(),
            source: "env",
            confirmed: true,
        };
    }
    if let Some(path) = var.and_then(|v| {
        shell
            .and_then(|s| s.as_ref().ok())
            .and_then(|vars| vars.get(v))
    }) {
        return Root {
            path: path.into(),
            source: "shell",
            confirmed: true,
        };
    }
    Root {
        path: default,
        source: "default",
        confirmed: shell.is_some_and(Result::is_ok),
    }
}
pub(super) fn load(context: &Context) -> Result<Value, String> {
    let path = context.app.join("hosts.json");
    safe_path(&path, &context.home)?;
    let value = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(|_| "宿主偏好文件损坏")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(_) => return Err("无法读取宿主偏好".into()),
    };
    if !value.is_object()
        || value.as_object().unwrap().values().any(|v| {
            !v.is_object()
                || v.get("root")
                    .is_some_and(|p| !p.as_str().is_some_and(|p| Path::new(p).is_absolute()))
        })
    {
        return Err("宿主偏好格式无效".into());
    }
    Ok(value)
}
pub(super) fn save(context: &Context, value: &Value) -> Result<(), String> {
    let path = context.app.join("hosts.json");
    safe_path(&path, &context.home)?;
    atomic(&path, &serde_json::to_vec_pretty(value).unwrap())
}
