//! Safety inspection and confirmation-digest computation for isolated directories.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::git::{
    GitRunner, WORKTREES_DIR_NAME, bytes_to_path, decode_git_path, parse_worktrees,
    porcelain_v2_paths, ref_exists, trim_line_end,
};
use super::leases::{is_link, is_same_or_subpath, same_dir};
use super::state::{nested_repositories, tree_fingerprint, worktree_link_valid};
use super::store::{MANAGED_BRANCH_PREFIX, Record, WorktreeStore};

const MAX_DISPLAY_FILES: usize = 10;
const MAX_DISPLAY_COMMITS: usize = 8;

pub(crate) const STATUS_ARGS: &[&str] = &[
    "status",
    "--porcelain=v2",
    "-z",
    "--untracked-files=all",
    "--ignore-submodules=all",
];

const IN_PROGRESS_MARKERS: &[&str] = &[
    "MERGE_HEAD",
    "REBASE_HEAD",
    "rebase-merge",
    "rebase-apply",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "sequencer",
];

pub(crate) struct Snapshot {
    pub head_oid: String,
    pub branch_oid: String,
    pub uncommitted: bool,
    pub uncommitted_files: Vec<String>,
    pub uncommitted_total: usize,
    pub unique_commits: bool,
    pub unmerged_commits: Vec<String>,
    pub unmerged_total: usize,
    pub nested: Vec<String>,
    pub orphan: bool,
    pub missing_dir: bool,
    pub rollback_only: bool,
    pub digest: String,
}

impl Snapshot {
    pub(crate) fn to_confirm_json(
        &self,
        status: &str,
        record: &Record,
        reason: Option<&str>,
    ) -> Value {
        json!({
            "id": record.id,
            "status": status,
            "path": record.worktree_path,
            "branch": record.branch(),
            "state": record.state,
            "attention": record.attention,
            "confirmation": self.digest,
            "head_oid": (!self.head_oid.is_empty()).then_some(&self.head_oid),
            "uncommitted": self.uncommitted,
            "uncommitted_files": self.uncommitted_files,
            "uncommitted_total": self.uncommitted_total,
            "unique_commits": self.unique_commits,
            "unmerged_commits": self.unmerged_commits,
            "unmerged_total": self.unmerged_total,
            "nested_repos": self.nested,
            "orphan": self.orphan,
            "missing_dir": self.missing_dir,
            "rollback_only": self.rollback_only,
            "reason": reason,
        })
    }
}

struct DigestInput<'a> {
    isolation_id: &'a str,
    workspace_id: &'a str,
    head_oid: &'a str,
    branch_oid: &'a str,
    usage_generation: i64,
    status_raw: &'a [u8],
    stat_bytes: &'a [u8],
    nested: &'a [String],
}

fn compute_digest(input: DigestInput<'_>) -> String {
    let joined_nested = input.nested.join("\n");
    let gen_text = input.usage_generation.to_string();
    let parts: [&[u8]; 8] = [
        input.isolation_id.as_bytes(),
        input.workspace_id.as_bytes(),
        input.head_oid.as_bytes(),
        input.branch_oid.as_bytes(),
        gen_text.as_bytes(),
        input.status_raw,
        input.stat_bytes,
        joined_nested.as_bytes(),
    ];
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(format!("{}:", part.len()).as_bytes());
        hasher.update(part);
        hasher.update(b";");
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn stat_fingerprint(wt_path: &Path, paths: &[String]) -> Vec<u8> {
    let unique: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    for rel in unique {
        let target = wt_path.join(rel);
        match fs::symlink_metadata(&target) {
            Err(_) => out.extend_from_slice(format!("{rel}\tmissing\n").as_bytes()),
            Ok(meta) => {
                let stamp = stat_line(&meta);
                out.extend_from_slice(format!("{rel}\t{stamp}\n").as_bytes());
            }
        }
    }
    out
}

fn stat_line(meta: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mtime = i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
        let ctime = i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec());
        format!("{}:{}:{mtime}:{ctime}", meta.mode(), meta.size())
    }
    #[cfg(not(unix))]
    {
        super::state::entry_stat(meta)
    }
}

pub(crate) fn verify_repo_identity(
    git: &GitRunner,
    record: &Record,
) -> Result<(PathBuf, PathBuf), String> {
    let main_root = PathBuf::from(&record.main_root);
    let common_dir = PathBuf::from(&record.repo_common_dir);
    if is_link(&main_root) || !main_root.is_dir() {
        return Err(format!("主工作树不存在或不安全：{}", main_root.display()));
    }
    if is_link(&common_dir) || !common_dir.is_dir() {
        return Err(format!(
            "Git 公共目录不存在或不安全：{}",
            common_dir.display()
        ));
    }
    let bare = git.run(
        &main_root,
        &["rev-parse", "--is-bare-repository"],
        &[0],
        &[],
    )?;
    if bare.stdout.trim_ascii() == b"true" {
        return Err("仓库已成为裸仓库".to_owned());
    }
    let actual = git.run(
        &main_root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        &[0],
        &[],
    )?;
    let actual_common = bytes_to_path(trim_line_end(&actual.stdout));
    if !same_dir(&common_dir, &actual_common) {
        return Err("主工作树不再属于原登记的仓库".to_owned());
    }
    let base_arg = format!("{}^{{commit}}", record.base_oid);
    git.run(
        &main_root,
        &["cat-file", "-e", &base_arg],
        &[0],
        &[("GIT_NO_LAZY_FETCH", "1")],
    )?;
    let container = main_root.join(WORKTREES_DIR_NAME);
    if is_link(&container) {
        return Err(format!(
            "隔离目录容器存在不安全链接：{}",
            container.display()
        ));
    }
    Ok((main_root, common_dir))
}

fn verify_worktree_identity(
    git: &GitRunner,
    store: &WorktreeStore,
    record: &Record,
    main_root: &Path,
    common_dir: &Path,
) -> Result<PathBuf, String> {
    let wt_path = PathBuf::from(&record.worktree_path);
    if is_link(&wt_path) || !wt_path.is_dir() {
        return Err(format!("隔离目录不存在或不安全：{}", wt_path.display()));
    }
    let Some(wt_parent) = wt_path.parent() else {
        return Err("隔离目录路径无效".to_owned());
    };
    if !same_dir(wt_parent, &main_root.join(WORKTREES_DIR_NAME)) {
        return Err("隔离目录不在 .agentnave-worktrees 下".to_owned());
    }
    let wt_common = bytes_to_path(trim_line_end(
        &git.run(
            &wt_path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            &[0],
            &[],
        )?
        .stdout,
    ));
    let wt_git_dir = bytes_to_path(trim_line_end(
        &git.run(
            &wt_path,
            &["rev-parse", "--path-format=absolute", "--git-dir"],
            &[0],
            &[],
        )?
        .stdout,
    ));
    if !same_dir(&wt_common, common_dir) {
        return Err("隔离目录指向了另一个 Git 公共目录".to_owned());
    }
    if record
        .worktree_git_dir
        .as_deref()
        .is_none_or(|dir| !same_dir(&wt_git_dir, Path::new(dir)))
    {
        return Err("隔离目录的 Git 管理目录与登记不符".to_owned());
    }
    for marker in IN_PROGRESS_MARKERS {
        let marker_path = wt_git_dir.join(marker);
        if marker_path.exists() || is_link(&marker_path) {
            return Err(format!("目录内有正在进行的 Git 操作（{marker}）"));
        }
    }
    verify_worktree_list(git, store, record, main_root, &wt_path)?;
    Ok(wt_path)
}

fn verify_worktree_list(
    git: &GitRunner,
    store: &WorktreeStore,
    record: &Record,
    main_root: &Path,
    wt_path: &Path,
) -> Result<(), String> {
    let worktrees = parse_worktrees(
        &git.run(
            main_root,
            &["worktree", "list", "--porcelain", "-z"],
            &[0],
            &[],
        )?
        .stdout,
    );
    if worktrees
        .first()
        .is_none_or(|first| !same_dir(&first.path, main_root))
    {
        return Err("git worktree list 中的主工作树身份已变化".to_owned());
    }
    let matches: Vec<_> = worktrees
        .iter()
        .filter(|w| w.path.exists() && same_dir(&w.path, wt_path))
        .collect();
    if matches.len() != 1 {
        return Err("隔离目录未唯一登记在 Git worktree 中".to_owned());
    }
    for entry in &worktrees {
        if std::ptr::eq(entry, matches[0]) {
            continue;
        }
        if entry.branch_ref.as_deref() == Some(record.branch_ref.as_str()) {
            return Err("受管分支正在另一个 worktree 中检出".to_owned());
        }
        if is_same_or_subpath(&entry.path, wt_path) {
            return Err("隔离目录内嵌套了另一个 Git worktree".to_owned());
        }
    }
    for other in store.all_records()? {
        if other.id != record.id && is_same_or_subpath(Path::new(&other.worktree_path), wt_path) {
            return Err(format!(
                "隔离目录内包含另一个已登记的隔离目录（{}）",
                other.id
            ));
        }
    }
    Ok(())
}

fn check_hidden_index_flags(git: &GitRunner, wt_path: &Path) -> Result<(), String> {
    let out = git
        .run(wt_path, &["ls-files", "-v", "-z"], &[0], &[])?
        .stdout;
    for item in out.split(|&b| b == 0) {
        let Some(&tag) = item.first() else {
            continue;
        };
        if tag.is_ascii_lowercase() {
            return Err("工作树包含 assume-unchanged 索引标记".to_owned());
        }
        if tag == b'S' || tag == b's' {
            return Err("工作树包含 skip-worktree 索引标记".to_owned());
        }
    }
    Ok(())
}

fn is_unique_commit(git: &GitRunner, cwd: &Path, oid: &str) -> Result<bool, String> {
    let contains = format!("--contains={oid}");
    let out = git
        .run(
            cwd,
            &[
                "for-each-ref",
                &contains,
                "--format=%(refname)",
                "refs/heads/",
                "refs/remotes/",
            ],
            &[0],
            &[],
        )?
        .stdout;
    let text = String::from_utf8_lossy(&out);
    let has_durable = text
        .lines()
        .any(|line| !line.is_empty() && !line.starts_with(MANAGED_BRANCH_PREFIX));
    Ok(!has_durable)
}

fn collect_unmerged_commits(
    git: &GitRunner,
    cwd: &Path,
    base_oid: &str,
    head_oid: &str,
) -> (Vec<String>, usize) {
    if head_oid.is_empty() || head_oid == base_oid {
        return (Vec::new(), 0);
    }
    let range = format!("{base_oid}..{head_oid}");
    let total = git
        .run(cwd, &["rev-list", "--count", &range], &[0], &[])
        .ok()
        .and_then(|out| String::from_utf8_lossy(&out.stdout).trim().parse().ok())
        .unwrap_or(0);
    let limit = format!("-{MAX_DISPLAY_COMMITS}");
    let commits = git
        .run(cwd, &["log", &limit, "--format=%h %s", &range], &[0], &[])
        .ok()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    (commits, total)
}

fn read_verified_head(
    git: &GitRunner,
    record: &Record,
    main_root: &Path,
    wt_path: &Path,
) -> Result<(String, String), String> {
    let sym = git.run(wt_path, &["symbolic-ref", "-q", "HEAD"], &[0, 1], &[])?;
    let actual_ref = if sym.code == 0 {
        decode_git_path(trim_line_end(&sym.stdout))
    } else {
        String::new()
    };
    if actual_ref != record.branch_ref {
        let found = if actual_ref.is_empty() {
            "detached HEAD"
        } else {
            &actual_ref
        };
        return Err(format!(
            "工作树 HEAD 不在受管分支 {} 上（当前为 {found}）",
            record.branch()
        ));
    }
    let head_oid = String::from_utf8_lossy(
        git.run(
            wt_path,
            &["rev-parse", "--verify", "HEAD^{commit}"],
            &[0],
            &[],
        )?
        .stdout
        .trim_ascii(),
    )
    .into_owned();
    let branch_target = format!("{}^{{commit}}", record.branch_ref);
    let branch_oid = String::from_utf8_lossy(
        git.run(
            main_root,
            &["rev-parse", "--verify", "--end-of-options", &branch_target],
            &[0],
            &[],
        )?
        .stdout
        .trim_ascii(),
    )
    .into_owned();
    if head_oid != branch_oid {
        return Err("受管分支尖端与工作树 HEAD 不一致".to_owned());
    }
    Ok((head_oid, branch_oid))
}

fn inspect_ready_locked(
    git: &GitRunner,
    store: &WorktreeStore,
    record: &Record,
    workspace_id: &str,
) -> Result<(PathBuf, PathBuf, Snapshot), String> {
    let (main_root, common_dir) = verify_repo_identity(git, record)?;
    let wt_path = verify_worktree_identity(git, store, record, &main_root, &common_dir)?;
    if !worktree_link_valid(&wt_path, record.worktree_git_dir.as_deref(), &common_dir) {
        return Err("隔离目录不再是该登记的有效 git worktree".to_owned());
    }
    let (head_oid, branch_oid) = read_verified_head(git, record, &main_root, &wt_path)?;
    check_hidden_index_flags(git, &wt_path)?;
    let status_raw = git.run(&wt_path, STATUS_ARGS, &[0], &[])?.stdout;
    let nested = nested_repositories(git, &wt_path, &status_raw)?;
    let unique_commits = is_unique_commit(git, &wt_path, &head_oid)?;
    let (unmerged_commits, unmerged_total) = if unique_commits {
        collect_unmerged_commits(git, &wt_path, &record.base_oid, &head_oid)
    } else {
        (Vec::new(), 0)
    };
    let paths = porcelain_v2_paths(&status_raw);
    let unique_paths: Vec<String> = paths
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .cloned()
        .collect();
    let uncommitted_total = unique_paths.len();
    let uncommitted_files = unique_paths.into_iter().take(MAX_DISPLAY_FILES).collect();
    let stat_bytes = stat_fingerprint(&wt_path, &paths);
    let digest = compute_digest(DigestInput {
        isolation_id: &record.id,
        workspace_id,
        head_oid: &head_oid,
        branch_oid: &branch_oid,
        usage_generation: record.usage_generation,
        status_raw: &status_raw,
        stat_bytes: &stat_bytes,
        nested: &nested,
    });
    Ok((
        main_root,
        common_dir,
        Snapshot {
            head_oid,
            branch_oid,
            uncommitted: !status_raw.is_empty(),
            uncommitted_files,
            uncommitted_total,
            unique_commits,
            unmerged_commits,
            unmerged_total,
            nested,
            orphan: false,
            missing_dir: false,
            rollback_only: false,
            digest,
        },
    ))
}

fn inspect_orphan_locked(
    git: &GitRunner,
    record: &Record,
    workspace_id: &str,
    main_root: &Path,
) -> Result<Snapshot, String> {
    let wt_path = PathBuf::from(&record.worktree_path);
    let Some(parent) = wt_path.parent() else {
        return Err("隔离目录路径无效".to_owned());
    };
    if wt_path.file_name().and_then(|n| n.to_str()) != Some(record.id.as_str())
        || !same_dir(parent, &main_root.join(WORKTREES_DIR_NAME))
    {
        return Err("隔离目录不在 .agentnave-worktrees 下".to_owned());
    }
    let mut tip = String::new();
    if ref_exists(git, main_root, &record.branch_ref)? {
        let target = format!("{}^{{commit}}", record.branch_ref);
        tip = String::from_utf8_lossy(
            git.run(
                main_root,
                &["rev-parse", "--verify", "--end-of-options", &target],
                &[0],
                &[],
            )?
            .stdout
            .trim_ascii(),
        )
        .into_owned();
    }
    let unique_commits = if tip.is_empty() {
        false
    } else {
        is_unique_commit(git, main_root, &tip)?
    };
    let (unmerged_commits, unmerged_total) = if unique_commits {
        collect_unmerged_commits(git, main_root, &record.base_oid, &tip)
    } else {
        (Vec::new(), 0)
    };
    let fp = tree_fingerprint(&wt_path);
    let digest = compute_digest(DigestInput {
        isolation_id: &record.id,
        workspace_id,
        head_oid: "orphan",
        branch_oid: if tip.is_empty() { "none" } else { &tip },
        usage_generation: record.usage_generation,
        status_raw: b"orphan",
        stat_bytes: &fp,
        nested: &[],
    });
    Ok(Snapshot {
        head_oid: String::new(),
        branch_oid: tip,
        uncommitted: true,
        uncommitted_files: Vec::new(),
        uncommitted_total: 0,
        unique_commits,
        unmerged_commits,
        unmerged_total,
        nested: Vec::new(),
        orphan: true,
        missing_dir: false,
        rollback_only: false,
        digest,
    })
}

pub(crate) fn inspect_record(
    git: &GitRunner,
    store: &WorktreeStore,
    record: &Record,
    workspace_id: &str,
) -> Result<(Option<(PathBuf, PathBuf)>, Snapshot), String> {
    let wt_path = PathBuf::from(&record.worktree_path);
    let dir_present = wt_path.exists() || is_link(&wt_path);

    if matches!(record.state.as_str(), "deleting" | "needs_attention") {
        if record.delete_oid.as_deref().is_none_or(str::is_empty) {
            let digest = compute_digest(DigestInput {
                isolation_id: &record.id,
                workspace_id,
                head_oid: "rollback",
                branch_oid: &record.base_oid,
                usage_generation: record.usage_generation,
                status_raw: b"rollback",
                stat_bytes: &[],
                nested: &[],
            });
            return Ok((
                None,
                Snapshot {
                    head_oid: String::new(),
                    branch_oid: record.base_oid.clone(),
                    uncommitted: false,
                    uncommitted_files: Vec::new(),
                    uncommitted_total: 0,
                    unique_commits: false,
                    unmerged_commits: Vec::new(),
                    unmerged_total: 0,
                    nested: Vec::new(),
                    orphan: false,
                    missing_dir: !dir_present,
                    rollback_only: true,
                    digest,
                },
            ));
        }
        if !dir_present {
            let (main_root, common_dir) = verify_repo_identity(git, record)?;
            let expected_oid = record.delete_oid.as_deref().unwrap_or("");
            let digest = compute_digest(DigestInput {
                isolation_id: &record.id,
                workspace_id,
                head_oid: "missing",
                branch_oid: expected_oid,
                usage_generation: record.usage_generation,
                status_raw: b"missing",
                stat_bytes: &[],
                nested: &[],
            });
            return Ok((
                Some((main_root, common_dir)),
                Snapshot {
                    head_oid: String::new(),
                    branch_oid: expected_oid.to_owned(),
                    uncommitted: false,
                    uncommitted_files: Vec::new(),
                    uncommitted_total: 0,
                    unique_commits: false,
                    unmerged_commits: Vec::new(),
                    unmerged_total: 0,
                    nested: Vec::new(),
                    orphan: false,
                    missing_dir: true,
                    rollback_only: false,
                    digest,
                },
            ));
        }
    } else if record.state != "ready" {
        return Err(format!("隔离目录当前状态为 {}，暂不能删除", record.state));
    }

    let (main_root, common_dir) = verify_repo_identity(git, record)?;
    let orphan = wt_path.is_dir()
        && !is_link(&wt_path)
        && !worktree_link_valid(&wt_path, record.worktree_git_dir.as_deref(), &common_dir);
    if orphan {
        let snap = inspect_orphan_locked(git, record, workspace_id, &main_root)?;
        Ok((Some((main_root, common_dir)), snap))
    } else {
        let (main_root, common_dir, snap) = inspect_ready_locked(git, store, record, workspace_id)?;
        Ok((Some((main_root, common_dir)), snap))
    }
}
