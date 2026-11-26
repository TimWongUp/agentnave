"""Git discovery, preflight validation, and worktree primitives for isolated directories."""

from __future__ import annotations

import atexit
import os
import re
import shutil
import signal
import stat
import subprocess
import tempfile
import threading
import time
from collections.abc import Mapping, Sequence
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from agentnave.isolated.store import ID_PATTERN
from agentnave.store import write_file

WORKTREES_DIR_NAME = ".agentnave-worktrees"
EXCLUDE_RULE = b"/.agentnave-worktrees/"
DEFAULT_GIT_TIMEOUT = 30.0
# Creating a worktree runs the repository's hooks and filters and may download LFS objects.
CREATE_GIT_TIMEOUT = 600.0
KILL_WAIT_SECONDS = 5.0
MAX_GIT_OUTPUT_BYTES = 16 * 1024 * 1024

_OID_RE = re.compile(r"^[0-9a-f]{40}(?:[0-9a-f]{24})?$")


class IsolationError(ValueError):
    """Repository state or request prevents safe isolated directory handling."""


@dataclass(frozen=True, slots=True)
class GitResult:
    returncode: int
    stdout: bytes
    stderr: str

    @property
    def line(self) -> bytes:
        """Output of a one-line command without its line end, and never more than that one.

        A path can itself end in CR or LF; trimming every trailing one would name another path.
        """
        line = self.stdout.removesuffix(b"\n")
        return line.removesuffix(b"\r") if os.name == "nt" else line


@dataclass(frozen=True, slots=True)
class WorktreeEntry:
    path: Path
    head_oid: str | None
    branch_ref: str | None
    bare: bool = False
    detached: bool = False


@dataclass(frozen=True, slots=True)
class RepoContext:
    cwd: Path
    source_root: Path
    main_root: Path
    repo_common_dir: Path
    repo_identity: str
    subdir_prefix: PurePosixPath


def is_link(path: Path) -> bool:
    return path.is_symlink() or os.path.isjunction(path)


def same_dir(a: Path, b: Path) -> bool:
    try:
        return os.path.samefile(a, b)
    except OSError:
        return False


def is_same_or_subpath(child: Path, parent: Path) -> bool:
    """True when child resolves to parent or inside parent, respecting APFS/NTFS casing."""
    parent_resolved = parent.resolve()
    child_resolved = child.resolve()
    if os.name == "nt":
        norm_parent = Path(os.path.normcase(str(parent_resolved)))
        norm_child = Path(os.path.normcase(str(child_resolved)))
        if norm_child.is_relative_to(norm_parent):
            return True
    elif child_resolved.is_relative_to(parent_resolved):
        return True
    if not parent_resolved.exists():
        return False
    parent_depth = len(parent_resolved.parts)
    for candidate in (child_resolved, *child_resolved.parents):
        if len(candidate.parts) < parent_depth:
            break
        if candidate.name.lower() == parent_resolved.name.lower() and same_dir(
            candidate, parent_resolved
        ):
            return True
    return False


def has_link_in_relpath(base: Path, rel: PurePosixPath) -> bool:
    """True if any existing component of rel under base is a symlink or Windows junction."""
    current = base
    for part in rel.parts:
        if part in ("", "."):
            continue
        current = current / part
        if is_link(current):
            return True
    return False


def repo_identity_key(repo_common_dir: Path) -> str:
    return os.path.normcase(str(repo_common_dir.resolve()))


def decode_git_path(raw: bytes) -> str:
    if os.name == "nt":
        return raw.decode("utf-8")
    return os.fsdecode(raw)


# Commands that never read file contents through attribute filters.
_NO_FILTER_COMMANDS = frozenset(
    {
        "config",
        "var",
        "rev-parse",
        "symbolic-ref",
        "show-ref",
        "for-each-ref",
        "ls-tree",
        "cat-file",
        "update-ref",
        "rev-list",
    }
)
_FILTER_PROPS_EMPTY = ("clean", "smudge", "process")

_hooks_dir_lock = threading.Lock()
_empty_hooks_dir: Path | None = None


def empty_hooks_dir() -> Path:
    """A service-owned empty directory that stands in for the repository's hooks."""
    global _empty_hooks_dir
    with _hooks_dir_lock:
        if _empty_hooks_dir is None or not _empty_hooks_dir.is_dir():
            created = Path(tempfile.mkdtemp(prefix="agentnave-empty-hooks-"))
            atexit.register(shutil.rmtree, created, ignore_errors=True)
            _empty_hooks_dir = created
        return _empty_hooks_dir


def filter_driver_names(cwd: Path) -> list[str]:
    """Names of all filter drivers in the effective config (reading config runs nothing)."""
    res = run_git(
        ["config", "--null", "--get-regexp", r"^filter\."],
        cwd=cwd,
        allowed_exit_codes=(0, 1),
    )
    names: set[str] = set()
    for item in res.stdout.split(b"\x00"):
        key = os.fsdecode(item.split(b"\n", 1)[0])
        if key.startswith("filter.") and key.count(".") >= 2:
            names.add(key[len("filter.") :].rsplit(".", 1)[0])
    return sorted(names)


def _filter_disable_args(names: Sequence[str]) -> list[str]:
    """`-c` options that empty each driver's commands and clear its required flag.

    Command-line options are applied after GIT_CONFIG_PARAMETERS and GIT_CONFIG_COUNT from the
    environment, so an inherited `filter.<name>.process` cannot override them. An empty command
    means no filtering; required=false keeps git from failing on that. Git splits `-c` at the
    first `=`, so a driver name holding one cannot be neutralized this way and is refused.
    """
    args: list[str] = []
    for name in names:
        if "=" in name:
            raise IsolationError(f"Unsupported filter driver name: {name!r}")
        for prop, value in (*((p, "") for p in _FILTER_PROPS_EMPTY), ("required", "false")):
            args.extend(["-c", f"filter.{name}.{prop}={value}"])
    return args


def control_char_in(text: str) -> bool:
    """True when text holds a control character (newline, escape, DEL, C1, ...)."""
    return any(ord(char) < 32 or 0x7F <= ord(char) < 0xA0 for char in text)


def run_git(
    args: Sequence[str],
    *,
    cwd: Path,
    allowed_exit_codes: Sequence[int] = (0,),
    extra_env: Mapping[str, str] | None = None,
    config_overrides: Sequence[str] = (),
    stdin: bytes | None = None,
    timeout: float = DEFAULT_GIT_TIMEOUT,
    max_output_bytes: int = MAX_GIT_OUTPUT_BYTES,
    disable_hooks: bool = True,
    neutralize_filters: bool = True,
    filters_from: Sequence[Path] = (),
) -> GitResult:
    """Run git as an argument array without a shell, bounded in time and output size.

    Every call neutralizes repository-configured executables: fsmonitor, hooks, external diff
    programs, signature verification (gpg.program), promisor lazy fetches and every filter driver
    (clean, smudge, process) in the effective configuration, whatever the attribute files say.
    Creating a worktree is the one exception: it passes disable_hooks=False and
    neutralize_filters=False, so hooks, filters, LFS and lazy fetches run as `git worktree add`
    runs them; such a run gets its own process group, killed whole on timeout.
    filters_from names other worktrees the command runs Git in (a non-forced `worktree remove`
    checks the target's status): the drivers in their own configuration are neutralized too.
    """
    cmd = ["git", "-C", str(cwd), "-c", "core.fsmonitor=false", "-c", "core.longpaths=true"]
    # Never recurse into submodules or nested repositories, whose own config would run there.
    for item in (
        "diff.ignoreSubmodules=all",
        "log.showSignature=false",
        "status.submoduleSummary=false",
        "submodule.recurse=false",
    ):
        cmd.extend(["-c", item])
    if disable_hooks:
        cmd.extend(["-c", f"core.hooksPath={empty_hooks_dir()}"])
    for item in config_overrides:
        cmd.extend(["-c", item])
    if neutralize_filters and args and args[0] not in _NO_FILTER_COMMANDS:
        names = set(filter_driver_names(cwd))
        for other in filters_from:
            names.update(filter_driver_names(other))
        cmd.extend(_filter_disable_args(sorted(names)))
    cmd.extend(args)
    env = os.environ.copy()
    # Also drop variables that point git at another config, repository, index or tree than
    # cwd: GIT_CONFIG would hide the repository's filters from filter_driver_names only.
    for name in (
        "GIT_EXTERNAL_DIFF",
        "GIT_PAGER",
        "GIT_ASKPASS",
        "GIT_CONFIG",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ):
        env.pop(name, None)
    env["GIT_TERMINAL_PROMPT"] = "0"
    if neutralize_filters:
        # A partial clone would otherwise fetch missing objects from its promisor remote.
        env["GIT_NO_LAZY_FETCH"] = "1"
    # Hooks and filters may start their own children; give them a group to kill on timeout.
    runs_repo_code = not disable_hooks or not neutralize_filters
    if extra_env:
        env.update(extra_env)
    try:
        proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            start_new_session=runs_repo_code and os.name != "nt",
        )
    except OSError as exc:
        raise IsolationError(f"Failed to execute git: {exc}") from exc
    try:
        stdout, stderr_bytes = proc.communicate(input=stdin, timeout=timeout)
    except subprocess.TimeoutExpired as exc:
        _kill_git(proc, tree=runs_repo_code)
        # Bounded: a surviving grandchild may keep the pipes open.
        with suppress(subprocess.TimeoutExpired):
            proc.communicate(timeout=KILL_WAIT_SECONDS)
        raise IsolationError(f"git {args[0] if args else ''} timed out after {timeout}s") from exc
    if len(stdout) > max_output_bytes:
        raise IsolationError(f"git {args[0] if args else ''} output exceeded {max_output_bytes}B")
    stderr = stderr_bytes.decode("utf-8", errors="replace").strip()
    if proc.returncode not in allowed_exit_codes:
        detail = stderr or f"exit status {proc.returncode}"
        raise IsolationError(f"git {' '.join(args[:2])} failed: {detail}")
    return GitResult(returncode=proc.returncode, stdout=stdout, stderr=stderr)


def _kill_git(proc: subprocess.Popen[bytes], *, tree: bool) -> None:
    """Kill git and, when it ran repository code, the hooks and filters it started."""
    if tree and os.name == "nt":
        with suppress(OSError, subprocess.SubprocessError):
            subprocess.run(
                ["taskkill", "/T", "/F", "/PID", str(proc.pid)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=KILL_WAIT_SECONDS,
                check=False,
            )
    elif tree:
        with suppress(OSError):
            os.killpg(proc.pid, signal.SIGKILL)
    proc.kill()


def parse_worktrees(porcelain_z: bytes) -> list[WorktreeEntry]:
    """Parse `git worktree list --porcelain -z` into structured entries."""
    entries: list[WorktreeEntry] = []
    path: Path | None = None
    head_oid: str | None = None
    branch_ref: str | None = None
    bare = False
    detached = False

    def flush() -> None:
        nonlocal path, head_oid, branch_ref, bare, detached
        if path is not None:
            entries.append(WorktreeEntry(path, head_oid, branch_ref, bare=bare, detached=detached))
        path = None
        head_oid = None
        branch_ref = None
        bare = False
        detached = False

    for token in porcelain_z.split(b"\x00"):
        if not token:
            flush()
            continue
        if token.startswith(b"worktree "):
            flush()
            path = Path(decode_git_path(token[len(b"worktree ") :]))
        elif token.startswith(b"HEAD "):
            head_oid = token[len(b"HEAD ") :].decode("ascii", errors="replace")
        elif token.startswith(b"branch "):
            branch_ref = decode_git_path(token[len(b"branch ") :])
        elif token == b"bare":
            bare = True
        elif token == b"detached":
            detached = True
    flush()
    return entries


def validate_cwd(cwd: Path | str) -> Path:
    raw = Path(cwd)
    if not raw.is_absolute():
        raise IsolationError("cwd must be an absolute path")
    resolved = raw.resolve()
    if not resolved.is_dir():
        raise IsolationError(f"cwd is not an existing directory: {cwd}")
    return resolved


def discover_repo(cwd: Path | str) -> RepoContext:
    """Resolve the enclosing Git repository and validate its main worktree."""
    resolved_cwd = validate_cwd(cwd)
    bare_out = run_git(["rev-parse", "--is-bare-repository"], cwd=resolved_cwd).stdout.strip()
    if bare_out == b"true":
        raise IsolationError("Bare Git repositories cannot use isolated directories")
    source_root = Path(
        decode_git_path(
            run_git(
                ["rev-parse", "--path-format=absolute", "--show-toplevel"], cwd=resolved_cwd
            ).line
        )
    ).resolve()
    common_dir = Path(
        decode_git_path(
            run_git(
                ["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=resolved_cwd
            ).line
        )
    ).resolve()
    prefix_raw = decode_git_path(run_git(["rev-parse", "--show-prefix"], cwd=resolved_cwd).line)
    prefix = PurePosixPath(prefix_raw.rstrip("/")) if prefix_raw.rstrip("/") else PurePosixPath()
    worktrees = parse_worktrees(
        run_git(["worktree", "list", "--porcelain", "-z"], cwd=resolved_cwd).stdout
    )
    if not worktrees or worktrees[0].bare:
        raise IsolationError("Bare or worktree-less Git repositories are not supported")
    main_root = worktrees[0].path.resolve()
    if not main_root.is_dir():
        raise IsolationError(f"Main worktree is missing: {main_root}")
    main_common = Path(
        decode_git_path(
            run_git(["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=main_root).line
        )
    ).resolve()
    if not same_dir(common_dir, main_common):
        raise IsolationError("Main worktree common-dir does not match repository common-dir")
    container = main_root / WORKTREES_DIR_NAME
    if is_link(container) or (container.exists() and not container.is_dir()):
        raise IsolationError(f"Unsafe {WORKTREES_DIR_NAME} container under {main_root}")
    return RepoContext(
        cwd=resolved_cwd,
        source_root=source_root,
        main_root=main_root,
        repo_common_dir=common_dir,
        repo_identity=repo_identity_key(common_dir),
        subdir_prefix=prefix,
    )


def resolve_target_commit(ctx: RepoContext, ref: str | None) -> str:
    """Resolve optional ref (default HEAD) to a full commit OID without option injection."""
    if ref is None:
        target = "HEAD"
    else:
        if not ref.strip():
            raise IsolationError("isolation ref must be a non-empty string")
        cleaned = ref.strip()
        if cleaned.startswith("-") or "\x00" in cleaned or "\n" in cleaned or "\r" in cleaned:
            raise IsolationError(f"Invalid isolation ref: {ref!r}")
        target = cleaned
    out = run_git(
        ["rev-parse", "--verify", "--end-of-options", f"{target}^{{commit}}"],
        cwd=ctx.source_root,
    ).stdout.strip()
    oid = out.decode("ascii", errors="replace")
    if not _OID_RE.fullmatch(oid):
        raise IsolationError(f"Resolved ref {target!r} did not yield a valid commit OID")
    return oid


def collect_creation_warnings(ctx: RepoContext) -> tuple[str, ...]:
    """Build user-facing warnings for uncopied dirty worktree state."""
    warnings: list[str] = []
    uncopied = _uncommitted_paths(ctx.source_root)
    if uncopied:
        shown = ", ".join(uncopied[:_UNCOPIED_SHOWN])
        more = len(uncopied) - _UNCOPIED_SHOWN
        warnings.append(
            "Uncommitted changes in the source worktree were not copied: "
            + (f"{shown} (+{more} more)." if more > 0 else f"{shown}.")
        )
    return tuple(warnings)


_UNCOPIED_SHOWN = 5


def _uncommitted_paths(source_root: Path) -> list[str]:
    """Changed and untracked paths of the worktree; an untracked directory counts once."""
    fields = run_git(
        ["status", "--porcelain=v1", "-z", "--untracked-files=normal", "--ignore-submodules=all"],
        cwd=source_root,
    ).stdout.split(b"\0")
    paths: list[str] = []
    renamed_from = False
    for field in fields:
        if renamed_from:  # a rename or copy is followed by the path it came from
            renamed_from = False
        elif len(field) > 3:
            paths.append(field[3:].decode(errors="replace"))
            renamed_from = field[0] in b"RC" or field[1] in b"RC"
    return paths


def ensure_info_exclude_locked(main_root: Path) -> None:
    """Idempotently append /.agentnave-worktrees/ to the repository's info/exclude."""
    exclude_out = run_git(
        ["rev-parse", "--path-format=absolute", "--git-path", "info/exclude"],
        cwd=main_root,
    ).line
    exclude_path = Path(decode_git_path(exclude_out))
    if is_link(exclude_path) or is_link(exclude_path.parent):
        raise IsolationError(f"Unsafe symlink or junction on info/exclude: {exclude_path}")
    exclude_path.parent.mkdir(parents=True, exist_ok=True)
    existing = b""
    if exclude_path.exists():
        info = exclude_path.lstat()
        if not stat.S_ISREG(info.st_mode):
            raise IsolationError(f"info/exclude is not a regular file: {exclude_path}")
        existing = exclude_path.read_bytes()
    if EXCLUDE_RULE in existing.splitlines():
        return
    prefix = existing if (not existing or existing.endswith(b"\n")) else existing + b"\n"
    write_file(exclude_path, prefix + EXCLUDE_RULE + b"\n")


def ref_exists(repo_dir: Path, full_ref: str) -> bool:
    res = run_git(
        ["show-ref", "--verify", "--quiet", full_ref],
        cwd=repo_dir,
        allowed_exit_codes=(0, 1),
    )
    return res.returncode == 0


def add_worktree_locked(
    main_root: Path,
    branch_name: str,
    worktree_path: Path,
    base_oid: str,
    *,
    create_branch: bool = True,
) -> Path:
    """Create the linked worktree on a managed branch, running hooks, filters and LFS as git does."""
    container = main_root / WORKTREES_DIR_NAME
    if is_link(container):
        raise IsolationError(f"Unsafe {WORKTREES_DIR_NAME} link: {container}")
    container.mkdir(parents=True, exist_ok=True)
    if is_link(container) or not container.is_dir():
        raise IsolationError(f"Invalid {WORKTREES_DIR_NAME} container: {container}")
    cmd = ["worktree", "add"]
    if create_branch:
        cmd.extend(["-b", branch_name, "--", str(worktree_path), base_oid])
    else:
        cmd.extend(["--", str(worktree_path), branch_name])
    run_git(
        cmd,
        cwd=main_root,
        disable_hooks=False,
        neutralize_filters=False,
        timeout=CREATE_GIT_TIMEOUT,
    )
    if is_link(worktree_path) or not worktree_path.is_dir():
        raise IsolationError(f"Created worktree directory is invalid: {worktree_path}")
    if filter_driver_names(worktree_path):
        _refresh_index_after_checkout(worktree_path)
    actual_head = (
        run_git(["rev-parse", "--verify", "HEAD^{commit}"], cwd=worktree_path)
        .stdout.strip()
        .decode("ascii", errors="replace")
    )
    actual_ref = decode_git_path(run_git(["symbolic-ref", "-q", "HEAD"], cwd=worktree_path).line)
    if actual_head != base_oid or actual_ref != f"refs/heads/{branch_name}":
        raise IsolationError("Created worktree HEAD or branch does not match target")
    git_dir = Path(
        decode_git_path(
            run_git(["rev-parse", "--path-format=absolute", "--git-dir"], cwd=worktree_path).line
        )
    ).resolve()
    return git_dir


def _refresh_index_after_checkout(worktree_path: Path) -> None:
    """Record checked-out stat data so inspection, which runs no filters, sees files as clean.

    Files written in the same second as the index stay racily clean, and Git re-reads them; with
    filters neutralized a smudged file then differs from its blob. Refreshing after the second
    turns over stores stat data Git trusts.
    """
    time.sleep(1.02 - time.time() % 1)
    run_git(
        ["update-index", "-q", "--refresh"],
        cwd=worktree_path,
        allowed_exit_codes=(0, 1),
        neutralize_filters=False,
        timeout=CREATE_GIT_TIMEOUT,
    )


def never_checked_out(worktree_path: Path) -> bool:
    """True while a worktree has no index yet, i.e. creation was interrupted during checkout; git
    then reports every tracked file as a staged deletion, which is not a change anyone made."""
    index = run_git(
        ["rev-parse", "--path-format=absolute", "--git-path", "index"], cwd=worktree_path
    ).line
    return not Path(decode_git_path(index)).exists()


def isolated_worktree_root(path: Path) -> Path | None:
    """Return the <main_root>/.agentnave-worktrees/<id> ancestor if path lies inside one."""
    resolved = path.resolve()
    for candidate in (resolved, *resolved.parents):
        # Case-insensitive, like the cheap pre-check on the path parts (APFS, NTFS).
        if (
            ID_PATTERN.fullmatch(candidate.name.lower())
            and candidate.parent.name.lower() == WORKTREES_DIR_NAME
        ):
            return candidate
    return None


def shares_cwd_tree(first: Path, second: Path) -> bool:
    """True if two execution cwds lie in the same isolated worktree or same ordinary dir tree."""
    iso_first = isolated_worktree_root(first)
    iso_second = isolated_worktree_root(second)
    if iso_first is not None or iso_second is not None:
        return iso_first is not None and iso_first == iso_second
    return is_same_or_subpath(first, second) or is_same_or_subpath(second, first)


def resolve_mapped_cwd(worktree_path: Path, subdir_prefix: PurePosixPath) -> Path:
    if not subdir_prefix.parts:
        return worktree_path.resolve()
    if has_link_in_relpath(worktree_path, subdir_prefix):
        raise IsolationError(
            f"Mapped cwd contains a symlink or junction component: {subdir_prefix.as_posix()}"
        )
    mapped = worktree_path.joinpath(*subdir_prefix.parts)
    if not mapped.is_dir():
        raise IsolationError(
            f"Target commit does not contain subdirectory: {subdir_prefix.as_posix()}"
        )
    resolved = mapped.resolve()
    if not is_same_or_subpath(resolved, worktree_path):
        raise IsolationError("Mapped cwd resolves outside the isolated worktree")
    return resolved
