"""What is on disk in an isolated directory: its Git link, nested repositories, leftovers."""

from __future__ import annotations

import hashlib
import os
import shutil
import stat
from collections.abc import Callable
from contextlib import suppress
from pathlib import Path

from agentnave.isolated.repo import (
    IsolationError,
    decode_git_path,
    is_link,
    run_git,
    same_dir,
)

MAX_FINGERPRINT_ENTRIES = 200_000


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
    """Gitlinks and untracked nested repositories, found without starting Git inside them."""
    nested: set[str] = set()
    listing = run_git(["ls-files", "-s", "-z"], cwd=wt_path).stdout
    for item in listing.split(b"\x00"):
        if item.startswith(b"160000 ") and b"\t" in item:
            nested.add(decode_git_path(item.split(b"\t", 1)[1]))
    for token in status_raw.split(b"\x00"):
        if token.startswith(b"? ") and token.endswith(b"/"):
            nested.add(decode_git_path(token[2:]).rstrip("/"))
    ignored, found = _ignored_nested_repositories(wt_path)
    nested.update(found)
    nested.update(_unreadable_directories(wt_path, ignored))
    return sorted(nested)


def _unreadable(wt_path: Path, found: set[str]) -> Callable[[OSError], None]:
    def record(error: OSError) -> None:
        # A directory that cannot be searched may hold a repository: ask, do not assume.
        if error.filename is not None:
            found.add(Path(error.filename).relative_to(wt_path).as_posix())

    return record


def _unreadable_directories(wt_path: Path, ignored: set[Path]) -> set[str]:
    """Directories outside the ignored ones that cannot be read: `status` only warns on them."""
    found: set[str] = set()
    for top, dirs, _ in os.walk(wt_path, onerror=_unreadable(wt_path, found)):
        here = Path(top)
        dirs[:] = [
            d
            for d in dirs
            if d != ".git" and not (here / d).is_symlink() and here / d not in ignored
        ]
    return found


def _ignored_nested_repositories(wt_path: Path) -> tuple[set[Path], set[str]]:
    """Ignored directories, and the repositories inside them, which `status` never lists."""
    found: set[str] = set()
    ignored: set[Path] = set()
    listing = run_git(
        ["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"],
        cwd=wt_path,
    ).stdout
    unreadable = _unreadable(wt_path, found)

    for item in listing.split(b"\x00"):
        if not item.endswith(b"/"):
            continue
        root = wt_path / decode_git_path(item).rstrip("/")
        ignored.add(root)
        for top, dirs, names in os.walk(root, onerror=unreadable):
            if ".git" in names or ".git" in dirs or _bare_repository(names, dirs):
                found.add(Path(top).relative_to(wt_path).as_posix())
                dirs.clear()
            else:
                dirs[:] = [d for d in dirs if not (Path(top) / d).is_symlink()]
    return ignored, found


def _bare_repository(names: list[str], dirs: list[str]) -> bool:
    """A bare repository has no .git entry, only its own HEAD, objects and refs."""
    return "HEAD" in names and "objects" in dirs and "refs" in dirs


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
