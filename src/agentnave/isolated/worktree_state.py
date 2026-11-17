"""What is on disk in an isolated directory: its Git link, nested repositories, leftovers."""

from __future__ import annotations

import hashlib
import os
import shutil
import stat
from collections import deque
from collections.abc import Callable
from contextlib import suppress
from pathlib import Path

from agentnave.isolated.repo import (
    WORKTREES_DIR_NAME,
    IsolationError,
    decode_git_path,
    is_link,
    run_git,
    same_dir,
)

MAX_FINGERPRINT_ENTRIES = 200_000


def verify_worktree_container(wt_path: Path, main_root: Path, isolation_id: str) -> None:
    """Refuse a path other than <main worktree>/.agentnave-worktrees/<isolation_id>.

    Every release, rollback and leftover cleanup checks this before deleting anything, so a
    record rewritten to point elsewhere never removes another directory.
    """
    parent = wt_path.parent
    if wt_path.name != isolation_id or parent.name != WORKTREES_DIR_NAME:
        raise IsolationError(
            f"{wt_path} is not .agentnave-worktrees/{isolation_id} of this repository"
        )
    container = main_root / WORKTREES_DIR_NAME
    if is_link(container) or is_link(parent):
        raise IsolationError(f"Unsafe link at {container}")
    if container.exists() or parent.exists():
        inside = same_dir(parent, container)
    else:
        inside = same_dir(parent.parent, main_root)
    if not inside:
        raise IsolationError(f"{wt_path} is not inside {container}")


def verify_worktree_root(wt_path: Path) -> None:
    """Refuse a worktree whose configuration (core.worktree) points Git at another directory.

    Status would then describe that other directory, and this one's changes would go unseen.
    """
    out = run_git(["rev-parse", "--path-format=absolute", "--show-toplevel"], cwd=wt_path)
    if not same_dir(Path(decode_git_path(out.line)), wt_path):
        raise IsolationError(f"Git uses another working tree than {wt_path} (core.worktree)")


def worktree_link_valid(wt_path: Path, admin_dir: str | None, common_dir: Path) -> bool:
    """True while wt_path/.git still points at this record's own admin directory.

    Git is only run inside a directory after this holds: a task can rewrite .git, and a .git
    pointing elsewhere would make Git read some other repository's configuration and hooks.
    """
    dotgit = wt_path / ".git"
    try:
        if is_link(dotgit) or not dotgit.is_file() or admin_dir is None:
            return False
        first = dotgit.read_text(encoding="utf-8", errors="replace").splitlines()[:1]
        if not first or not first[0].startswith("gitdir:"):
            return False
        pointed = Path(first[0][len("gitdir:") :].strip())
        if not pointed.is_absolute():
            pointed = wt_path / pointed
        admin = Path(admin_dir)
        if not pointed.is_dir() or not same_dir(pointed, admin):
            return False
        if not same_dir(admin.parent, common_dir / "worktrees"):
            return False
        back = (admin / "gitdir").read_text(encoding="utf-8", errors="replace").strip()
        return same_dir(Path(back).parent, wt_path)
    except (OSError, ValueError):
        return False


def foreign_link_valid(wt_path: Path) -> bool:
    """The same check for a directory this data root has no registration of."""
    dotgit = wt_path / ".git"
    try:
        if is_link(dotgit) or not dotgit.is_file():
            return False
        first = dotgit.read_text(encoding="utf-8", errors="replace").splitlines()[:1]
        if not first or not first[0].startswith("gitdir:"):
            return False
        pointed = Path(first[0][len("gitdir:") :].strip())
        if not pointed.is_absolute():
            pointed = wt_path / pointed
        if not pointed.is_dir() or pointed.parent.name != "worktrees":
            return False
        back = (pointed / "gitdir").read_text(encoding="utf-8", errors="replace").strip()
        return same_dir(Path(back).parent, wt_path)
    except (OSError, ValueError):
        return False


def nested_repositories(wt_path: Path, status_raw: bytes) -> list[str]:
    """Gitlinks and nested repositories anywhere in the tree, found without starting Git in them.

    The walk covers tracked, untracked and ignored directories alike: `status` does not list a
    repository inside a tracked directory, nor a bare repository, nor an ignored one.
    """
    nested: set[str] = set()
    listing = run_git(["ls-files", "-s", "-z"], cwd=wt_path).stdout
    for item in listing.split(b"\x00"):
        if item.startswith(b"160000 ") and b"\t" in item:
            nested.add(decode_git_path(item.split(b"\t", 1)[1]))
    for token in status_raw.split(b"\x00"):
        if token.startswith(b"? ") and token.endswith(b"/"):
            nested.add(decode_git_path(token[2:]).rstrip("/"))
    nested.update(_walk_nested(wt_path))
    return sorted(nested)


def _walk_nested(wt_path: Path) -> set[str]:
    """Directories holding a .git entry or a bare repository layout, and unreadable directories.

    Breadth first, never following links and never descending into a repository found. Names
    compare without case: Git finds `.Git` on case-insensitive file systems.
    """
    found: set[str] = set()
    pending = deque([wt_path])
    while pending:
        directory = pending.popleft()
        try:
            entries = list(os.scandir(directory))
        except OSError:
            # A directory that cannot be searched may hold a repository: ask, do not assume.
            found.add(directory.relative_to(wt_path).as_posix())
            continue
        subdirs: list[Path] = []
        dir_names: set[str] = set()
        file_names: set[str] = set()
        has_dotgit = False
        for entry in entries:
            name = entry.name.lower()
            if name == ".git":
                has_dotgit = True
                continue
            try:
                if entry.is_symlink() or entry.is_junction():
                    if name == "head":
                        file_names.add(name)
                    elif name in {"objects", "refs"}:
                        dir_names.add(name)
                    continue
                if entry.is_dir(follow_symlinks=False):
                    subdirs.append(Path(entry.path))
                    dir_names.add(name)
                elif entry.is_file(follow_symlinks=False):
                    file_names.add(name)
            except OSError:
                continue
        bare = "head" in file_names and {"objects", "refs"} <= dir_names
        if directory != wt_path and (has_dotgit or bare):
            found.add(directory.relative_to(wt_path).as_posix())
        else:
            pending.extend(subdirs)
    return found


def tree_fingerprint(root: Path) -> bytes:
    """Digest of every entry under root (names, types, sizes, times), never following links."""
    hasher = hashlib.sha256()
    count = 0
    pending = [root]
    while pending:
        directory = pending.pop()
        try:
            entries = sorted(os.scandir(directory), key=lambda entry: entry.name)
        except OSError as exc:
            hasher.update(f"unreadable:{directory}:{exc.errno}\n".encode())
            continue
        for entry in entries:
            count += 1
            if count > MAX_FINGERPRINT_ENTRIES:
                hasher.update(b"truncated\n")
                return hasher.digest()
            try:
                info = entry.stat(follow_symlinks=False)
            except OSError:
                hasher.update(f"missing:{entry.path}\n".encode())
                continue
            relative = os.path.relpath(entry.path, root)
            hasher.update(
                f"{relative}\t{info.st_mode}:{info.st_size}:{info.st_mtime_ns}\n".encode(
                    "utf-8", errors="surrogateescape"
                )
            )
            if stat.S_ISDIR(info.st_mode) and not entry.is_symlink():
                pending.append(Path(entry.path))
    return hasher.digest()


def remove_tree(path: Path, container: Path) -> None:
    """Delete a leftover directory under container without following links; fix read-only bits.

    Raises IsolationError naming the path when something cannot be removed.
    """
    if is_link(path) or not path.is_dir():
        raise IsolationError(f"Refusing to remove {path}: not a plain directory")
    if not same_dir(path.parent, container) or path.parent.is_symlink():
        raise IsolationError(f"Refusing to remove {path}: not directly under {container}")

    def make_writable_and_retry(
        function: Callable[[str], object], target: str, _exc: object
    ) -> None:
        for item in (os.path.dirname(target), target):
            # chmod follows links: never touch one, or its target outside the tree changes.
            if os.path.islink(item) or os.path.isjunction(item):
                continue
            with suppress(OSError):
                os.chmod(item, os.stat(item, follow_symlinks=False).st_mode | stat.S_IRWXU)
        function(target)

    try:
        shutil.rmtree(path, onexc=make_writable_and_retry)
    except OSError as exc:
        raise IsolationError(
            f"Could not remove {path} ({exc}); remove this directory manually"
        ) from exc
    if path.exists():
        raise IsolationError(f"Could not remove {path}; remove this directory manually")
