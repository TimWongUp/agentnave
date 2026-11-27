//! Git execution with hooks, filters, and fsmonitor neutralized (ADR-0085, ADR-0098).

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use tempfile::TempDir;

use crate::process::{OutputError, output};

pub(crate) const WORKTREES_DIR_NAME: &str = ".agentnave-worktrees";
const DEFAULT_GIT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_GIT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

const NO_FILTER_COMMANDS: &[&str] = &[
    "config",
    "var",
    "rev-parse",
    "symbolic-ref",
    "show-ref",
    "for-each-ref",
    "ls-tree",
    "cat-file",
    "update-ref",
    "log",
    "rev-list",
];

const REMOVED_ENVS: &[&str] = &[
    "GIT_EXTERNAL_DIFF",
    "GIT_PAGER",
    "GIT_ASKPASS",
    "GIT_CONFIG",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
];

pub(crate) struct GitOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct WorktreeEntry {
    pub path: PathBuf,
    pub head_oid: Option<String>,
    pub branch_ref: Option<String>,
}

pub(crate) struct GitRunner {
    empty_hooks: TempDir,
}

impl GitRunner {
    pub(crate) fn new() -> Result<Self, String> {
        let empty_hooks = tempfile::Builder::new()
            .prefix("agentnave-empty-hooks-")
            .tempdir_in(crate::temp_dir::usable())
            .map_err(|e| format!("无法创建临时 hooks 目录：{e}"))?;
        Ok(Self { empty_hooks })
    }

    pub(crate) fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        allowed_exit_codes: &[i32],
        extra_env: &[(&str, &str)],
    ) -> Result<GitOutput, String> {
        self.run_with_timeout(
            cwd,
            args,
            allowed_exit_codes,
            extra_env,
            DEFAULT_GIT_TIMEOUT,
        )
    }

    pub(crate) fn run_with_timeout(
        &self,
        cwd: &Path,
        args: &[&str],
        allowed_exit_codes: &[i32],
        extra_env: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<GitOutput, String> {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        self.run_os_with_timeout(cwd, &args, allowed_exit_codes, extra_env, timeout)
    }

    pub(crate) fn run_os(
        &self,
        cwd: &Path,
        args: &[&OsStr],
        allowed_exit_codes: &[i32],
        extra_env: &[(&str, &str)],
    ) -> Result<GitOutput, String> {
        self.run_os_with_timeout(
            cwd,
            args,
            allowed_exit_codes,
            extra_env,
            DEFAULT_GIT_TIMEOUT,
        )
    }

    pub(crate) fn run_os_with_timeout(
        &self,
        cwd: &Path,
        args: &[&OsStr],
        allowed_exit_codes: &[i32],
        extra_env: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<GitOutput, String> {
        self.run_os_inner(cwd, args, allowed_exit_codes, extra_env, timeout, &[])
    }

    /// Like `run`, also neutralizing the filter drivers configured in `filters_from`: a
    /// non-forced `worktree remove` checks the target's status under that worktree's own
    /// configuration (worktree config, `includeIf "onbranch:..."`).
    pub(crate) fn run_filters_from(
        &self,
        cwd: &Path,
        args: &[&str],
        allowed_exit_codes: &[i32],
        filters_from: &[&Path],
    ) -> Result<GitOutput, String> {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        self.run_os_inner(
            cwd,
            &args,
            allowed_exit_codes,
            &[],
            DEFAULT_GIT_TIMEOUT,
            filters_from,
        )
    }

    fn run_os_inner(
        &self,
        cwd: &Path,
        args: &[&OsStr],
        allowed_exit_codes: &[i32],
        extra_env: &[(&str, &str)],
        timeout: Duration,
        filters_from: &[&Path],
    ) -> Result<GitOutput, String> {
        let mut cmd = Command::new("git");
        cmd.stdin(Stdio::null());
        cmd.current_dir(crate::temp_dir::usable());
        crate::temp_dir::strip_child_env(&mut cmd);
        crate::process::hide_window(&mut cmd);
        cmd.arg("--no-pager").arg("-C").arg(cwd);
        for item in [
            "core.fsmonitor=false",
            "core.longpaths=true",
            "diff.external=",
            "diff.ignoreSubmodules=all",
            "log.showSignature=false",
            "status.submoduleSummary=false",
            // `git worktree remove` checks for untracked files through `status`; a user's
            // `showUntrackedFiles=no` would let it delete files written after our own check.
            "status.showUntrackedFiles=normal",
            "submodule.recurse=false",
        ] {
            cmd.args(["-c", item]);
        }
        cmd.arg("-c").arg(format!(
            "core.hooksPath={}",
            self.empty_hooks.path().display()
        ));
        if args.first().is_some_and(|first| {
            !NO_FILTER_COMMANDS
                .iter()
                .any(|name| *first == OsStr::new(name))
        }) {
            let mut names = BTreeSet::new();
            for dir in std::iter::once(cwd).chain(filters_from.iter().copied()) {
                names.extend(self.filter_driver_names(dir)?);
            }
            for name in names {
                if name.contains('=') {
                    return Err(format!("不支持的 Git filter 驱动名：{name}"));
                }
                for prop in ["clean", "smudge", "process"] {
                    cmd.args(["-c", &format!("filter.{name}.{prop}=")]);
                }
                cmd.args(["-c", &format!("filter.{name}.required=false")]);
            }
        }
        cmd.args(args);
        for name in REMOVED_ENVS {
            cmd.env_remove(name);
        }
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.env("GIT_NO_LAZY_FETCH", "1");
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let out = match output(cmd, timeout) {
            Ok(out) => out,
            Err(OutputError::Timeout) => {
                let op = args.first().copied().unwrap_or_default().to_string_lossy();
                return Err(format!("git {op} 执行超时"));
            }
            Err(OutputError::Io(e)) => return Err(format!("无法执行 git：{e}")),
            Err(OutputError::Reader) => return Err("读取 git 输出失败".to_owned()),
        };
        if out.stdout.len() > MAX_GIT_OUTPUT_BYTES {
            let op = args.first().copied().unwrap_or_default().to_string_lossy();
            return Err(format!("git {op} 输出超出上限"));
        }
        let code = out.status.code().unwrap_or(-1);
        if !allowed_exit_codes.contains(&code) {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            let detail = match (stderr.is_empty(), stdout.is_empty()) {
                (false, _) => stderr,
                (true, false) => stdout,
                (true, true) => format!("exit status {code}"),
            };
            let op = args
                .iter()
                .take(2)
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            return Err(format!("git {op} failed: {detail}"));
        }
        Ok(GitOutput {
            code,
            stdout: out.stdout,
        })
    }

    fn filter_driver_names(&self, cwd: &Path) -> Result<Vec<String>, String> {
        let res = self.run(
            cwd,
            &["config", "--null", "--get-regexp", r"^filter\."],
            &[0, 1],
            &[("GIT_OPTIONAL_LOCKS", "0")],
        )?;
        let mut names = BTreeSet::new();
        for item in res.stdout.split(|&b| b == 0) {
            if item.is_empty() {
                continue;
            }
            let key_bytes = item.split(|&b| b == b'\n').next().unwrap_or_default();
            let key = std::str::from_utf8(key_bytes).map_err(|_| {
                "Git filter 配置键包含非 UTF-8 字节，无法安全禁用，已拒绝操作".to_owned()
            })?;
            if let Some(rest) = key.strip_prefix("filter.")
                && let Some((driver, _prop)) = rest.rsplit_once('.')
            {
                names.insert(driver.to_owned());
            }
        }
        Ok(names.into_iter().collect())
    }
}

pub(crate) fn bytes_to_path(raw: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(OsStr::from_bytes(raw))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(raw).as_ref())
    }
}

pub(crate) fn decode_git_path(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// Output of a one-line git command without its line end, and never more than that one: a path
/// can itself end in CR or LF, and trimming every trailing one would name another path.
pub(crate) fn trim_line_end(raw: &[u8]) -> &[u8] {
    let line = raw.strip_suffix(b"\n").unwrap_or(raw);
    if cfg!(windows) {
        line.strip_suffix(b"\r").unwrap_or(line)
    } else {
        line
    }
}

pub(crate) fn parse_worktrees(porcelain_z: &[u8]) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut head_oid: Option<String> = None;
    let mut branch_ref: Option<String> = None;
    let flush = |entries: &mut Vec<WorktreeEntry>,
                 path: &mut Option<PathBuf>,
                 head_oid: &mut Option<String>,
                 branch_ref: &mut Option<String>| {
        if let Some(p) = path.take() {
            entries.push(WorktreeEntry {
                path: p,
                head_oid: head_oid.take(),
                branch_ref: branch_ref.take(),
            });
        } else {
            *head_oid = None;
            *branch_ref = None;
        }
    };
    for token in porcelain_z.split(|&b| b == 0) {
        if token.is_empty() {
            flush(&mut entries, &mut path, &mut head_oid, &mut branch_ref);
            continue;
        }
        if let Some(rest) = token.strip_prefix(b"worktree ") {
            flush(&mut entries, &mut path, &mut head_oid, &mut branch_ref);
            path = Some(bytes_to_path(rest));
        } else if let Some(rest) = token.strip_prefix(b"HEAD ") {
            head_oid = Some(String::from_utf8_lossy(rest).into_owned());
        } else if let Some(rest) = token.strip_prefix(b"branch ") {
            branch_ref = Some(decode_git_path(rest));
        }
    }
    flush(&mut entries, &mut path, &mut head_oid, &mut branch_ref);
    entries
}

/// Extract all reported relative paths from `git status --porcelain=v2 -z`.
pub(crate) fn porcelain_v2_paths(status_z: &[u8]) -> Vec<String> {
    let mut tokens: Vec<&[u8]> = status_z.split(|&b| b == 0).collect();
    if tokens.last() == Some(&b"".as_slice()) {
        tokens.pop();
    }
    let mut all_paths = Vec::new();
    let mut idx = 0;
    while idx < tokens.len() {
        let token = tokens[idx];
        if let Some(rest) = token.strip_prefix(b"? ") {
            all_paths.push(decode_git_path(rest));
            idx += 1;
        } else if token.starts_with(b"1 ") {
            let parts: Vec<&[u8]> = token.splitn(9, |&b| b == b' ').collect();
            if parts.len() == 9 {
                all_paths.push(decode_git_path(parts[8]));
            }
            idx += 1;
        } else if token.starts_with(b"2 ") {
            let parts: Vec<&[u8]> = token.splitn(10, |&b| b == b' ').collect();
            if parts.len() == 10 {
                all_paths.push(decode_git_path(parts[9]));
            }
            if idx + 1 < tokens.len() {
                all_paths.push(decode_git_path(tokens[idx + 1]));
            }
            idx += 2;
        } else if token.starts_with(b"u ") {
            let parts: Vec<&[u8]> = token.splitn(11, |&b| b == b' ').collect();
            if parts.len() == 11 {
                all_paths.push(decode_git_path(parts[10]));
            }
            idx += 1;
        } else {
            idx += 1;
        }
    }
    all_paths
}

pub(crate) fn ref_exists(git: &GitRunner, repo_dir: &Path, full_ref: &str) -> Result<bool, String> {
    let res = git.run(
        repo_dir,
        &["show-ref", "--verify", "--quiet", "--", full_ref],
        &[0, 1],
        &[("GIT_OPTIONAL_LOCKS", "0")],
    )?;
    Ok(res.code == 0)
}

pub(crate) fn never_checked_out(git: &GitRunner, wt_path: &Path) -> Result<bool, String> {
    let res = git.run(
        wt_path,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        &[0],
        &[],
    )?;
    let index = bytes_to_path(trim_line_end(&res.stdout));
    Ok(!index.exists())
}
