"""Git discovery, preflight validation, and worktree primitives for isolated directories."""

from __future__ import annotations

import atexit
import os
import re
import shutil
import stat
import subprocess
import tempfile
import threading
from collections.abc import Mapping, Sequence
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from agentnave.isolated.store import ID_PATTERN
from agentnave.store import write_file

WORKTREES_DIR_NAME = ".agentnave-worktrees"
EXCLUDE_RULE = b"/.agentnave-worktrees/"
DEFAULT_GIT_TIMEOUT = 30.0
KILL_WAIT_SECONDS = 5.0
MAX_GIT_OUTPUT_BYTES = 16 * 1024 * 1024

_OID_RE = re.compile(r"^[0-9a-f]{40}(?:[0-9a-f]{24})?$")
_FILTER_ATTR_RE = re.compile(r"(?:^|\s)filter=([^\s#]+)")


class IsolationError(ValueError):
    """Repository state or request prevents safe isolated directory handling."""


@dataclass(frozen=True, slots=True)
class GitResult:
    returncode: int
    stdout: bytes
    stderr: str


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


def _disable_filter_drivers(env: dict[str, str], names: Sequence[str]) -> None:
    """Add GIT_CONFIG_* entries that empty each driver's commands and clear its required flag.

    An empty command means no filtering; required=false keeps git from failing on that.
    """
    if not names:
        return
    try:
        count = int(env.get("GIT_CONFIG_COUNT", "0") or 0)
    except ValueError:
        count = 0
    for name in names:
        for prop, value in (*((p, "") for p in _FILTER_PROPS_EMPTY), ("required", "false")):
            env[f"GIT_CONFIG_KEY_{count}"] = f"filter.{name}.{prop}"
            env[f"GIT_CONFIG_VALUE_{count}"] = value
            count += 1
    env["GIT_CONFIG_COUNT"] = str(count)


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
) -> GitResult:
    """Run git as an argument array without a shell, bounded in time and output size.

    Every call neutralizes repository-configured executables: fsmonitor, hooks, external diff
    programs and every filter driver (clean, smudge, process) in the effective configuration,
    whatever the attribute files say. Pass disable_hooks=False only for commands that read hook
    configuration.
    """
    cmd = ["git", "-C", str(cwd), "-c", "core.fsmonitor=false", "-c", "core.longpaths=true"]
    # Never recurse into submodules or nested repositories, whose own config would run there.
    for item in (
        "diff.ignoreSubmodules=all",
        "status.submoduleSummary=false",
        "submodule.recurse=false",
    ):
        cmd.extend(["-c", item])
    if disable_hooks:
        cmd.extend(["-c", f"core.hooksPath={empty_hooks_dir()}"])
    for item in config_overrides:
        cmd.extend(["-c", item])
    cmd.extend(args)
    env = os.environ.copy()
    for name in ("GIT_EXTERNAL_DIFF", "GIT_PAGER", "GIT_ASKPASS"):
        env.pop(name, None)
    env["GIT_TERMINAL_PROMPT"] = "0"
    if extra_env:
        env.update(extra_env)
    if args and args[0] not in _NO_FILTER_COMMANDS:
        _disable_filter_drivers(env, filter_driver_names(cwd))
    try:
        proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
    except OSError as exc:
        raise IsolationError(f"Failed to execute git: {exc}") from exc
    try:
        stdout, stderr_bytes = proc.communicate(input=stdin, timeout=timeout)
    except subprocess.TimeoutExpired as exc:
        proc.kill()
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
            ).stdout.rstrip(b"\r\n")
        )
    ).resolve()
    common_dir = Path(
        decode_git_path(
            run_git(
                ["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=resolved_cwd
            ).stdout.rstrip(b"\r\n")
        )
    ).resolve()
    prefix_raw = decode_git_path(
        run_git(["rev-parse", "--show-prefix"], cwd=resolved_cwd).stdout.rstrip(b"\r\n")
    )
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
            run_git(
                ["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=main_root
            ).stdout.rstrip(b"\r\n")
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
        extra_env={"GIT_NO_LAZY_FETCH": "1"},
    ).stdout.strip()
    oid = out.decode("ascii", errors="replace")
    if not _OID_RE.fullmatch(oid):
        raise IsolationError(f"Resolved ref {target!r} did not yield a valid commit OID")
    return oid


def check_tree_and_filters(ctx: RepoContext, base_oid: str) -> bool:
    """Reject submodules (160000) and custom non-LFS filter drivers; report LFS presence.

    Filters are looked up in every place Git reads attributes from, because checkout and status
    run any configured smudge/clean command. When a source cannot be read, isolation is refused.
    """
    no_fetch = {"GIT_NO_LAZY_FETCH": "1"}
    tree = run_git(
        ["ls-tree", "-r", "-z", "--full-tree", base_oid],
        cwd=ctx.main_root,
        extra_env=no_fetch,
    ).stdout
    for entry in tree.split(b"\x00"):
        if entry.startswith(b"160000 "):
            raise IsolationError(
                "Target commit contains Git submodules (160000); isolated directories reject them"
            )
    referenced = _referenced_filters(_attribute_texts(ctx, base_oid))
    custom_drivers = _configured_custom_filters(ctx.source_root)
    unsupported = sorted(referenced & custom_drivers)
    if unsupported:
        raise IsolationError(
            "Repository configures unsupported custom Git filter driver(s) referenced in "
            f"Git attributes: {', '.join(unsupported)}"
        )
    return "lfs" in referenced


def _attribute_texts(ctx: RepoContext, base_oid: str) -> str:
    """Text of every attribute source Git may consult for the target tree and source worktree."""
    no_fetch = {"GIT_NO_LAZY_FETCH": "1"}
    if os.environ.get("GIT_ATTR_SOURCE"):
        raise IsolationError("GIT_ATTR_SOURCE is set; attribute sources cannot be verified")
    attr_tree = run_git(
        ["config", "--get", "attr.tree"], cwd=ctx.source_root, allowed_exit_codes=(0, 1)
    )
    if attr_tree.returncode == 0:
        raise IsolationError("attr.tree is configured; attribute sources cannot be verified")
    pathspec = ["--", ".gitattributes", "**/.gitattributes"]
    texts = [
        run_git(
            ["grep", "-h", "-e", "filter=", base_oid, *pathspec],
            cwd=ctx.source_root,
            allowed_exit_codes=(0, 1),
            extra_env=no_fetch,
        ).stdout,
        run_git(
            ["grep", "-h", "--untracked", "-e", "filter=", *pathspec],
            cwd=ctx.source_root,
            allowed_exit_codes=(0, 1),
            extra_env=no_fetch,
        ).stdout,
    ]
    for path in _attribute_files(ctx):
        try:
            if path.is_file():
                texts.append(path.read_bytes())
            elif path.exists():
                raise IsolationError(f"Attribute source is not a regular file: {path}")
        except OSError as exc:
            raise IsolationError(f"Cannot read attribute source {path}: {exc}") from exc
    return "\n".join(text.decode("utf-8", errors="replace") for text in texts)


def _attribute_files(ctx: RepoContext) -> list[Path]:
    """Repository-local, configured global, default global and system attribute files."""
    found: list[Path] = []
    info = run_git(
        ["rev-parse", "--path-format=absolute", "--git-path", "info/attributes"],
        cwd=ctx.main_root,
    ).stdout.rstrip(b"\r\n")
    found.append(Path(decode_git_path(info)))
    configured = run_git(
        ["config", "--path", "--get", "core.attributesfile"],
        cwd=ctx.source_root,
        allowed_exit_codes=(0, 1),
    )
    if configured.returncode == 0 and configured.stdout.strip():
        path = Path(decode_git_path(configured.stdout.rstrip(b"\r\n")))
        if path.is_absolute():
            found.append(path)
        else:
            # Git resolves a relative path against the worktree it runs in.
            found.extend([ctx.source_root / path, ctx.main_root / path])
    for variable, fallback in (
        ("GIT_ATTR_GLOBAL", _default_global_attributes()),
        ("GIT_ATTR_SYSTEM", None),
    ):
        reported = run_git(["var", variable], cwd=ctx.source_root, allowed_exit_codes=(0, 128))
        if reported.returncode == 0 and reported.stdout.strip():
            found.append(Path(decode_git_path(reported.stdout.rstrip(b"\r\n"))))
        elif fallback is not None:
            found.append(fallback)
    return found


def _default_global_attributes() -> Path:
    xdg = os.environ.get("XDG_CONFIG_HOME")
    base = Path(xdg) if xdg else Path.home() / ".config"
    return base / "git" / "attributes"


def _referenced_filters(attributes_text: str) -> set[str]:
    names: set[str] = set()
    for line in attributes_text.splitlines():
        stripped = line.lstrip()
        if not stripped or stripped.startswith("#"):
            continue
        names.update(_FILTER_ATTR_RE.findall(stripped))
    return names


def _configured_custom_filters(source_root: Path) -> set[str]:
    res = run_git(
        ["config", "--null", "--get-regexp", r"^filter\."],
        cwd=source_root,
        allowed_exit_codes=(0, 1),
    )
    if res.returncode == 1 or not res.stdout:
        return set()
    drivers: set[str] = set()
    for raw in res.stdout.split(b"\x00"):
        if not raw:
            continue
        text = raw.decode("utf-8", errors="replace")
        if "\n" not in text:
            continue
        key, value = text.split("\n", 1)
        if not key.startswith("filter.") or not value.strip():
            continue
        rest = key[len("filter.") :]
        parts = rest.rsplit(".", 1)
        if len(parts) != 2:
            continue
        driver, prop = parts
        if prop in ("smudge", "clean", "process") and driver != "lfs":
            drivers.add(driver)
    return drivers


def _has_active_hooks(source_root: Path) -> bool:
    cfg = run_git(
        ["config", "--get", "core.hooksPath"],
        cwd=source_root,
        allowed_exit_codes=(0, 1),
        disable_hooks=False,
    )
    if cfg.returncode == 0 and cfg.stdout.strip():
        raw_hooks = Path(decode_git_path(cfg.stdout.rstrip(b"\r\n"))).expanduser()
        hooks_dir = raw_hooks if raw_hooks.is_absolute() else (source_root / raw_hooks)
    else:
        hooks_out = run_git(
            ["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
            cwd=source_root,
            disable_hooks=False,
        ).stdout.rstrip(b"\r\n")
        hooks_dir = Path(decode_git_path(hooks_out))
    if not hooks_dir.is_dir():
        return False
    try:
        entries = list(hooks_dir.iterdir())
    except OSError:
        return True
    for entry in entries:
        if entry.name.endswith(".sample") or entry.name.startswith("."):
            continue
        try:
            mode = entry.stat().st_mode
        except OSError:
            continue
        if not stat.S_ISREG(mode):
            continue
        if os.name == "nt" or os.access(entry, os.X_OK):
            return True
    return False


def collect_creation_warnings(ctx: RepoContext, *, lfs_detected: bool) -> tuple[str, ...]:
    """Build user-facing warnings for skipped hooks/LFS and uncopied dirty worktree state."""
    warnings: list[str] = []
    hooks_skipped = _has_active_hooks(ctx.source_root)
    if hooks_skipped and lfs_detected:
        warnings.append("Git hooks and LFS downloads were skipped; LFS files may contain pointers.")
    elif hooks_skipped:
        warnings.append("Git hooks were skipped when creating the isolated directory.")
    elif lfs_detected:
        warnings.append("Git LFS downloads were skipped; LFS files may contain pointers.")
    dirty = run_git(
        ["status", "--porcelain=v2", "-z", "--untracked-files=all", "--ignore-submodules=all"],
        cwd=ctx.source_root,
    ).stdout
    if dirty:
        warnings.append("Uncommitted changes in the source worktree were not copied.")
    return tuple(warnings)


def ensure_info_exclude_locked(main_root: Path) -> None:
    """Idempotently append /.agentnave-worktrees/ to the repository's info/exclude."""
    exclude_out = run_git(
        ["rev-parse", "--path-format=absolute", "--git-path", "info/exclude"],
        cwd=main_root,
    ).stdout.rstrip(b"\r\n")
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
) -> Path:
    """Create the linked worktree on a brand-new managed branch with hooks and LFS disabled."""
    container = main_root / WORKTREES_DIR_NAME
    if is_link(container):
        raise IsolationError(f"Unsafe {WORKTREES_DIR_NAME} link: {container}")
    container.mkdir(parents=True, exist_ok=True)
    if is_link(container) or not container.is_dir():
        raise IsolationError(f"Invalid {WORKTREES_DIR_NAME} container: {container}")
    run_git(
        ["worktree", "add", "-b", branch_name, "--", str(worktree_path), base_oid],
        cwd=main_root,
        extra_env={
            "GIT_LFS_SKIP_SMUDGE": "1",
            "GIT_NO_LAZY_FETCH": "1",
        },
    )
    if is_link(worktree_path) or not worktree_path.is_dir():
        raise IsolationError(f"Created worktree directory is invalid: {worktree_path}")
    actual_head = (
        run_git(["rev-parse", "--verify", "HEAD^{commit}"], cwd=worktree_path)
        .stdout.strip()
        .decode("ascii", errors="replace")
    )
    actual_ref = decode_git_path(
        run_git(["symbolic-ref", "-q", "HEAD"], cwd=worktree_path).stdout.rstrip(b"\r\n")
    )
    if actual_head != base_oid or actual_ref != f"refs/heads/{branch_name}":
        raise IsolationError("Created worktree HEAD or branch does not match target")
    git_dir = Path(
        decode_git_path(
            run_git(
                ["rev-parse", "--path-format=absolute", "--git-dir"], cwd=worktree_path
            ).stdout.rstrip(b"\r\n")
        )
    ).resolve()
    return git_dir


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
