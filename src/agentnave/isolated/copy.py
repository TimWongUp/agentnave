"""Bounded, link-safe copying of Git-ignored files matched by .worktreeinclude."""

from __future__ import annotations

import os
import stat
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from agentnave.isolated.repo import (
    WORKTREES_DIR_NAME,
    IsolationError,
    decode_git_path,
    has_link_in_relpath,
    is_link,
    run_git,
)

INCLUDE_FILENAME = ".worktreeinclude"
MAX_INCLUDE_FILES = 1000
MAX_INCLUDE_FILE_BYTES = 32 * 1024 * 1024
MAX_INCLUDE_TOTAL_BYTES = 200 * 1024 * 1024
_COPY_CHUNK_BYTES = 64 * 1024


@dataclass(slots=True)
class _CopyBudget:
    files: int = 0
    total_bytes: int = 0


def _matched_ignored_paths(source_root: Path, include_file: Path) -> list[PurePosixPath]:
    matched = run_git(
        [
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            f"--exclude-from={include_file}",
        ],
        cwd=source_root,
    ).stdout
    if not matched:
        return []
    ignored = run_git(
        ["check-ignore", "-z", "--stdin"],
        cwd=source_root,
        allowed_exit_codes=(0, 1),
        stdin=matched,
    )
    if ignored.returncode == 1 or not ignored.stdout:
        return []
    seen: set[str] = set()
    result: list[PurePosixPath] = []
    for raw in ignored.stdout.split(b"\x00"):
        if not raw or raw.endswith(b"/"):
            continue
        decoded = decode_git_path(raw)
        if decoded in seen:
            continue
        seen.add(decoded)
        rel = PurePosixPath(decoded)
        if not rel.parts or rel.is_absolute() or ".." in rel.parts or "." in rel.parts:
            continue
        if ".git" in rel.parts or WORKTREES_DIR_NAME in rel.parts:
            continue
        result.append(rel)
    result.sort()
    return result


def _inside_nested_repo(source_root: Path, rel: PurePosixPath) -> bool:
    current = source_root
    for part in rel.parts[:-1]:
        current = current / part
        if (current / ".git").exists() or is_link(current / ".git"):
            return True
    return False


def _ensure_safe_parent_dirs(dest_root: Path, rel: PurePosixPath) -> Path:
    current = dest_root
    for part in rel.parts[:-1]:
        current = current / part
        if is_link(current):
            raise IsolationError(f"Destination path component is a link: {current}")
        if not current.exists():
            current.mkdir(mode=0o755)
        elif not current.is_dir():
            raise IsolationError(f"Destination parent is not a directory: {current}")
    return current / rel.parts[-1]


def _copy_single_file(
    src: Path,
    dst: Path,
    expected_stat: os.stat_result,
    budget: _CopyBudget,
) -> None:
    open_flags = os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    src_fd = os.open(src, open_flags)
    try:
        opened = os.fstat(src_fd)
        if not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino) != (
            expected_stat.st_dev,
            expected_stat.st_ino,
        ):
            raise IsolationError(f"Source file changed during .worktreeinclude copy: {src}")
        mode = (opened.st_mode & 0o777) or 0o600
        dst_flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
        dst_fd = os.open(dst, dst_flags, mode)
        written_file = 0
        try:
            while True:
                chunk = os.read(src_fd, _COPY_CHUNK_BYTES)
                if not chunk:
                    break
                written_file += len(chunk)
                budget.total_bytes += len(chunk)
                if written_file > MAX_INCLUDE_FILE_BYTES:
                    raise IsolationError(
                        f".worktreeinclude file exceeds {MAX_INCLUDE_FILE_BYTES} bytes: {src.name}"
                    )
                if budget.total_bytes > MAX_INCLUDE_TOTAL_BYTES:
                    raise IsolationError(
                        f".worktreeinclude total size exceeds {MAX_INCLUDE_TOTAL_BYTES} bytes"
                    )
                view = memoryview(chunk)
                offset = 0
                while offset < len(view):
                    offset += os.write(dst_fd, view[offset:])
        finally:
            os.close(dst_fd)
    finally:
        os.close(src_fd)


def copy_worktreeinclude(source_root: Path, dest_root: Path) -> int:
    """Copy ignored regular files matched by <source_root>/.worktreeinclude into dest_root."""
    include_file = source_root / INCLUDE_FILENAME
    if is_link(include_file):
        return 0
    try:
        include_stat = include_file.lstat()
    except FileNotFoundError:
        return 0
    if not stat.S_ISREG(include_stat.st_mode):
        raise IsolationError(f"{INCLUDE_FILENAME} must be a regular file")

    candidates = _matched_ignored_paths(source_root, include_file)
    budget = _CopyBudget()
    for rel in candidates:
        if has_link_in_relpath(source_root, rel) or has_link_in_relpath(dest_root, rel):
            continue
        if _inside_nested_repo(source_root, rel):
            continue
        src = source_root.joinpath(*rel.parts)
        try:
            src_stat = src.lstat()
        except FileNotFoundError:
            continue
        if not stat.S_ISREG(src_stat.st_mode):
            continue
        dst_candidate = dest_root.joinpath(*rel.parts)
        if dst_candidate.exists() or is_link(dst_candidate):
            continue
        budget.files += 1
        if budget.files > MAX_INCLUDE_FILES:
            raise IsolationError(f".worktreeinclude matched more than {MAX_INCLUDE_FILES} files")
        if src_stat.st_size > MAX_INCLUDE_FILE_BYTES:
            raise IsolationError(
                f".worktreeinclude file exceeds {MAX_INCLUDE_FILE_BYTES} bytes: {rel.as_posix()}"
            )
        if budget.total_bytes + src_stat.st_size > MAX_INCLUDE_TOTAL_BYTES:
            raise IsolationError(
                f".worktreeinclude total size exceeds {MAX_INCLUDE_TOTAL_BYTES} bytes"
            )
        dst = _ensure_safe_parent_dirs(dest_root, rel)
        if dst.exists() or is_link(dst):
            budget.files -= 1
            continue
        _copy_single_file(src, dst, src_stat, budget)
    return budget.files
