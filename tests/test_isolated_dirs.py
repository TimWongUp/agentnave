"""Real-Git integration tests for service-managed isolated directories (Phase 1)."""

from __future__ import annotations

import os
import subprocess
from dataclasses import replace
from pathlib import Path

import pytest

from agentnave.isolated import (
    MAX_SUMMARY_FILES,
    IsolatedDirs,
    IsolationError,
    IsolationOwnershipError,
)
from agentnave.store import now


def _git(cwd: Path, *args: str) -> str:
    res = subprocess.run(
        [
            "git",
            "-C",
            str(cwd),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            *args,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    return res.stdout.strip()


def _init_repo(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    _git(path, "init", "-b", "main")
    (path / ".gitignore").write_text("*.ignored\nignored_dir/\n", encoding="utf-8")
    (path / "README.md").write_text("base\n", encoding="utf-8")
    _git(path, "add", ".gitignore", "README.md")
    _git(path, "commit", "-m", "initial")
    return path


def test_ignored_artifacts_removed_only_on_explicit_release_and_exclude_is_idempotent(
    tmp_path: Path,
) -> None:
    repo = _init_repo(tmp_path / "repo")
    dirs = IsolatedDirs(tmp_path / "data")
    assert dirs.restart_safe()

    created = dirs.create(repo, workspace_id="ws-1")
    created.lease.release()

    artifact = created.worktree_path / "build.ignored"
    artifact.write_text("compiled output\n", encoding="utf-8")

    # Ignored files remain intact until explicit release, and do not require a confirmation token.
    assert artifact.is_file()
    items = dirs.list_for_workspace("ws-1")
    assert len(items) == 1 and not items[0].in_use
    assert dirs.unreleased_count("ws-1") == 1

    exclude_path = Path(
        _git(repo, "rev-parse", "--path-format=absolute", "--git-path", "info/exclude")
    )
    second = dirs.create(repo, workspace_id="ws-1")
    second.lease.release()
    exclude_lines = [
        line
        for line in exclude_path.read_text(encoding="utf-8").splitlines()
        if line == "/.agentnave-worktrees/"
    ]
    assert len(exclude_lines) == 1

    res = dirs.release(created.id, workspace_id="ws-1")
    assert res.status == "deleted"
    assert not created.worktree_path.exists()
    assert f"agentnave/{created.id}" not in _git(
        repo, "for-each-ref", "--format=%(refname:short)", "refs/heads/"
    )
    assert dirs.unreleased_count("ws-1") == 1
    assert dirs.release(created.id, workspace_id="ws-1").status == "not_found"
    assert dirs.release(second.id, workspace_id="ws-1").status == "deleted"
    assert dirs.unreleased_count("ws-1") == 0
    dirs.close()


def test_uncommitted_and_unique_commits_require_fresh_confirmation_token(tmp_path: Path) -> None:
    repo = _init_repo(tmp_path / "repo")
    dirs = IsolatedDirs(tmp_path / "data")

    # 1. Uncommitted tracked/untracked modifications require confirmation and expire on content edits.
    wt1 = dirs.create(repo, workspace_id="ws-1")
    wt1.lease.release()
    tracked = wt1.worktree_path / "README.md"
    tracked.write_text("draft-v1\n", encoding="utf-8")

    first_check = dirs.release(wt1.id, workspace_id="ws-1")
    assert first_check.status == "confirmation_required"
    assert first_check.uncommitted is True
    assert first_check.confirmation is not None
    assert wt1.worktree_path.is_dir()

    # Editing the already-modified file without changing HEAD must invalidate the prior token.
    tracked.write_text("draft-v2-longer-content\n", encoding="utf-8")
    stale_attempt = dirs.release(wt1.id, workspace_id="ws-1", confirmation=first_check.confirmation)
    assert stale_attempt.status == "confirmation_required"
    assert stale_attempt.confirmation is not None
    assert stale_attempt.confirmation != first_check.confirmation
    assert wt1.worktree_path.is_dir()

    # Claiming a new task lease bumps usage_generation and also invalidates the token.
    claimed = dirs.claim_for_cwd(wt1.worktree_path)
    assert claimed is not None
    claimed.lease.release()
    gen_stale = dirs.release(wt1.id, workspace_id="ws-1", confirmation=stale_attempt.confirmation)
    assert gen_stale.status == "confirmation_required"
    assert gen_stale.confirmation is not None

    deleted = dirs.release(wt1.id, workspace_id="ws-1", confirmation=gen_stale.confirmation)
    assert deleted.status == "deleted"
    assert not wt1.worktree_path.exists()

    # 2. Two managed branches pointing to the same unique commit must NOT vouch for each other.
    wt2 = dirs.create(repo, workspace_id="ws-1")
    wt2.lease.release()
    (wt2.worktree_path / "unique.txt").write_text("only in managed branches\n", encoding="utf-8")
    _git(wt2.worktree_path, "add", "unique.txt")
    _git(wt2.worktree_path, "commit", "-m", "unique work")
    unique_oid = _git(wt2.worktree_path, "rev-parse", "HEAD")

    wt3 = dirs.create(wt2.worktree_path, workspace_id="ws-1")
    wt3.lease.release()
    assert wt3.base_oid == unique_oid

    needs_confirm = dirs.release(wt2.id, workspace_id="ws-1")
    assert needs_confirm.status == "confirmation_required"
    assert needs_confirm.unique_commits is True
    assert needs_confirm.uncommitted is False
    assert needs_confirm.confirmation is not None

    # Once a non-managed user branch contains that commit, release succeeds without confirmation.
    _git(repo, "branch", "saved-work", unique_oid)
    assert dirs.release(wt2.id, workspace_id="ws-1").status == "deleted"
    assert dirs.release(wt3.id, workspace_id="ws-1").status == "deleted"
    dirs.close()


def test_flat_layout_from_isolated_dir_and_subdir_mapping_and_user_branch_safety(
    tmp_path: Path,
) -> None:
    repo = _init_repo(tmp_path / "repo")
    sub = repo / "packages" / "app"
    sub.mkdir(parents=True)
    (sub / "index.ts").write_text("export const x = 1;\n", encoding="utf-8")
    _git(repo, "checkout", "-b", "feature/existing")
    _git(repo, "add", "packages/app/index.ts")
    _git(repo, "commit", "-m", "add app subdir")
    feature_oid = _git(repo, "rev-parse", "HEAD")
    _git(repo, "checkout", "main")

    dirs = IsolatedDirs(tmp_path / "data")

    # Creating from ref="feature/existing" creates a new managed branch, never checking out or deleting feature/existing.
    first = dirs.create(repo, workspace_id="ws-1", ref="feature/existing")
    first.lease.release()
    assert first.base_oid == feature_oid
    assert first.branch == f"agentnave/{first.id}"
    assert first.worktree_path.parent == (repo / ".agentnave-worktrees").resolve()

    # Dispatching from inside an existing isolated dir's subdirectory stays flat under main_root/.agentnave-worktrees.
    nested_call = dirs.create(first.worktree_path / "packages" / "app", workspace_id="ws-1")
    nested_call.lease.release()
    assert nested_call.worktree_path.parent == (repo / ".agentnave-worktrees").resolve()
    assert nested_call.execution_cwd == nested_call.worktree_path / "packages" / "app"
    assert dirs.find_by_path(nested_call.execution_cwd) is not None

    # Target ref lacking the requested subdirectory fails cleanly and leaves zero leftovers.
    before_branches = _git(repo, "for-each-ref", "--format=%(refname)", "refs/heads/")
    with pytest.raises(IsolationError, match="subdirectory"):
        dirs.create(first.worktree_path / "packages" / "app", workspace_id="ws-1", ref="main")
    assert _git(repo, "for-each-ref", "--format=%(refname)", "refs/heads/") == before_branches
    assert dirs.unreleased_count("ws-1") == 2

    assert dirs.release(first.id, workspace_id="ws-1").status == "deleted"
    assert dirs.release(nested_call.id, workspace_id="ws-1").status == "deleted"
    # User branch feature/existing is completely untouched.
    assert _git(repo, "rev-parse", "refs/heads/feature/existing") == feature_oid
    dirs.close()


def test_rejects_option_refs_unborn_bare_submodules_and_custom_filters_and_emits_warnings(
    tmp_path: Path,
) -> None:
    dirs = IsolatedDirs(tmp_path / "data")

    # 1. Unborn (no-commit) repository rejected.
    unborn = tmp_path / "unborn"
    unborn.mkdir()
    _git(unborn, "init", "-b", "main")
    with pytest.raises(IsolationError):
        dirs.create(unborn, workspace_id="ws-1")

    # 2. Bare repository rejected.
    bare = tmp_path / "bare.git"
    bare.mkdir()
    _git(bare, "init", "--bare")
    with pytest.raises(IsolationError, match="[Bb]are"):
        dirs.create(bare, workspace_id="ws-1")

    # 3. Option-like ref (e.g. --orphan, -q) rejected without creating anything.
    repo = _init_repo(tmp_path / "repo")
    for bad_ref in ("--orphan", "-q", ""):
        with pytest.raises(IsolationError):
            dirs.create(repo, workspace_id="ws-1", ref=bad_ref)
    assert dirs.unreleased_count("ws-1") == 0

    # 4. Hooks + LFS + dirty source worktree produce warnings without running hooks or failing LFS.
    hook_marker = tmp_path / "hook-ran"
    hook = repo / ".git" / "hooks" / "post-checkout"
    hook.write_text(f"#!/bin/sh\ntouch '{hook_marker}'\nexit 1\n", encoding="utf-8")
    hook.chmod(0o755)
    (repo / ".gitattributes").write_text(
        "*.bin filter=lfs diff=lfs merge=lfs -text\n", encoding="utf-8"
    )
    _git(repo, "add", ".gitattributes")
    _git(repo, "commit", "-m", "enable lfs")
    _git(repo, "config", "filter.lfs.process", "nonexistent-git-lfs filter-process")
    _git(repo, "config", "filter.lfs.required", "true")
    (repo / "dirty.txt").write_text("unsaved\n", encoding="utf-8")

    warned = dirs.create(repo, workspace_id="ws-1")
    warned.lease.release()
    assert not hook_marker.exists()
    joined_warnings = " | ".join(warned.warnings)
    assert "hooks" in joined_warnings.lower()
    assert "lfs" in joined_warnings.lower()
    assert "Uncommitted changes" in joined_warnings
    assert dirs.release(warned.id, workspace_id="ws-1").status == "deleted"

    # 5. Custom non-LFS filter referenced in .gitattributes is rejected.
    (repo / "dirty.txt").unlink()
    _git(repo, "config", "filter.lfs.process", "")
    _git(repo, "config", "filter.lfs.required", "false")
    _git(repo, "config", "filter.secret.smudge", "cat")
    (repo / "sub").mkdir()
    (repo / "sub" / ".gitattributes").write_text("*.enc filter=secret\n", encoding="utf-8")
    _git(repo, "add", "sub/.gitattributes")
    _git(repo, "commit", "-m", "custom filter")
    with pytest.raises(IsolationError, match="secret"):
        dirs.create(repo, workspace_id="ws-1")

    # 6. Target commit with submodule (mode 160000) is rejected.
    sub_repo = _init_repo(tmp_path / "sub_repo")
    sub_oid = _git(sub_repo, "rev-parse", "HEAD")
    clean_repo = _init_repo(tmp_path / "with_submod")
    _git(clean_repo, "update-index", "--add", "--cacheinfo", f"160000,{sub_oid},vendor/sub")
    _git(clean_repo, "commit", "-m", "add gitlink")
    with pytest.raises(IsolationError, match="160000|submodule"):
        dirs.create(clean_repo, workspace_id="ws-1")
    dirs.close()


def test_leases_and_hard_blocks_prevent_release_even_with_confirmation(tmp_path: Path) -> None:
    repo = _init_repo(tmp_path / "repo")
    dirs_a = IsolatedDirs(tmp_path / "data_a")
    dirs_b = IsolatedDirs(tmp_path / "data_b")

    created = dirs_a.create(repo, workspace_id="ws-owner")
    # Initial creation lease is still held: release is blocked.
    assert dirs_a.list_for_workspace("ws-owner")[0].in_use is True
    assert dirs_a.release(created.id, workspace_id="ws-owner").status == "blocked"
    created.lease.release()

    # Cross-data-root borrower claiming a subdirectory lease blocks owner release and cannot release itself.
    sub = created.worktree_path / "sub"
    sub.mkdir()
    borrowed = dirs_b.claim_for_cwd(sub)
    assert borrowed is not None and borrowed.id == created.id
    with pytest.raises(IsolationOwnershipError):
        dirs_a.release(created.id, workspace_id="ws-borrower")
    assert dirs_a.release(created.id, workspace_id="ws-owner").status == "blocked"
    borrowed.lease.release()

    # Obtain a valid confirmation token while dirty, then verify hard blocks cannot be bypassed by it.
    (created.worktree_path / "README.md").write_text("changed\n", encoding="utf-8")
    tok_res = dirs_a.release(created.id, workspace_id="ws-owner")
    assert tok_res.status == "confirmation_required" and tok_res.confirmation is not None
    token = tok_res.confirmation

    # assume-unchanged blocks release even with token.
    _git(created.worktree_path, "checkout", "--", "README.md")
    _git(created.worktree_path, "update-index", "--assume-unchanged", "README.md")
    assert (
        dirs_a.release(created.id, workspace_id="ws-owner", confirmation=token).status == "blocked"
    )
    _git(created.worktree_path, "update-index", "--no-assume-unchanged", "README.md")

    # skip-worktree blocks release.
    _git(created.worktree_path, "update-index", "--skip-worktree", "README.md")
    assert (
        dirs_a.release(created.id, workspace_id="ws-owner", confirmation=token).status == "blocked"
    )
    _git(created.worktree_path, "update-index", "--no-skip-worktree", "README.md")

    # In-progress Git operation marker in worktree git-dir blocks release.
    wt_git_dir = Path(
        _git(created.worktree_path, "rev-parse", "--path-format=absolute", "--git-dir")
    )
    merge_head = wt_git_dir / "MERGE_HEAD"
    merge_head.write_text(created.base_oid + "\n", encoding="utf-8")
    assert dirs_a.release(created.id, workspace_id="ws-owner").status == "blocked"
    merge_head.unlink()

    # Detached HEAD blocks release.
    _git(created.worktree_path, "checkout", "--detach", "HEAD")
    assert dirs_a.release(created.id, workspace_id="ws-owner").status == "blocked"
    _git(created.worktree_path, "checkout", created.branch)

    assert dirs_a.release(created.id, workspace_id="ws-owner").status == "deleted"
    dirs_a.close()
    dirs_b.close()


def test_worktreeinclude_copies_only_ignored_matches_skips_links_and_rolls_back_on_limit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _init_repo(tmp_path / "repo")
    (repo / ".gitignore").write_text(".env*\nignored_dir/\n", encoding="utf-8")
    _git(repo, "add", ".gitignore")
    _git(repo, "commit", "-m", "update gitignore")

    (repo / ".worktreeinclude").write_text(
        ".env*\n!.env.secret\nnot_gitignored.txt\nREADME.md\nignored_dir/**\n",
        encoding="utf-8",
    )
    (repo / ".env.local").write_text("PORT=3000\n", encoding="utf-8")
    (repo / ".env.secret").write_text("SECRET=1\n", encoding="utf-8")
    (repo / "not_gitignored.txt").write_text("untracked not ignored\n", encoding="utf-8")
    (repo / "ignored_dir" / "nested").mkdir(parents=True)
    (repo / "ignored_dir" / "nested" / "cache.json").write_text("{}\n", encoding="utf-8")

    outside = tmp_path / "outside_secret.txt"
    outside.write_text("do not copy\n", encoding="utf-8")
    if os.name != "nt":
        (repo / ".env.symlink").symlink_to(outside)
        (repo / "ignored_dir" / "link_dir").symlink_to(tmp_path)

    dirs = IsolatedDirs(tmp_path / "data")
    created = dirs.create(repo, workspace_id="ws-1")
    created.lease.release()

    assert (created.worktree_path / ".env.local").read_text(encoding="utf-8") == "PORT=3000\n"
    assert (created.worktree_path / "ignored_dir" / "nested" / "cache.json").read_text(
        encoding="utf-8"
    ) == "{}\n"
    assert not (created.worktree_path / ".env.secret").exists()
    assert not (created.worktree_path / "not_gitignored.txt").exists()
    assert not (created.worktree_path / ".env.symlink").exists()
    assert not (created.worktree_path / "ignored_dir" / "link_dir").exists()
    assert (created.worktree_path / "README.md").read_text(encoding="utf-8") == "base\n"
    assert dirs.release(created.id, workspace_id="ws-1").status == "deleted"

    # Exceeding .worktreeinclude file-size limit fails and completely rolls back worktree, branch, and DB row.
    import agentnave.isolated.copy as copy_mod

    monkeypatch.setattr(copy_mod, "MAX_INCLUDE_FILE_BYTES", 4)
    branches_before = _git(repo, "for-each-ref", "--format=%(refname)", "refs/heads/")
    with pytest.raises(IsolationError, match="exceeds"):
        dirs.create(repo, workspace_id="ws-1")
    assert _git(repo, "for-each-ref", "--format=%(refname)", "refs/heads/") == branches_before
    assert list((repo / ".agentnave-worktrees").iterdir()) == []
    assert dirs.store.all_records() == []
    dirs.close()


def test_recovery_and_change_summary_edge_cases(tmp_path: Path) -> None:
    repo = _init_repo(tmp_path / "repo")
    dirs = IsolatedDirs(tmp_path / "data")

    # 1. Summarize cumulative changes, truncation (>200), and non-raising error path.
    wt = dirs.create(repo, workspace_id="ws-1")
    wt.lease.release()
    for idx in range(MAX_SUMMARY_FILES + 5):
        (wt.worktree_path / f"f_{idx:03d}.txt").write_text("x\n", encoding="utf-8")
    summary = dirs.summarize_changes(wt.id)
    assert summary.summary_status == "complete"
    assert summary.truncated is True
    assert len(summary.changed_files_from_base) == MAX_SUMMARY_FILES
    assert summary.uncommitted is True
    assert summary.actual_branch == wt.branch

    missing_summary = dirs.summarize_changes("wt-000000000000")
    assert missing_summary.summary_status == "error"
    assert missing_summary.error is not None

    # Clean up the untracked files and test interrupted `deleting` recovery.
    for path in wt.worktree_path.glob("f_*.txt"):
        path.unlink()
    dirs.store.mark_deleting(
        wt.id, phase="removing_worktree", delete_oid=wt.base_oid, updated_at=now()
    )
    outcomes = dirs.recover()
    assert len(outcomes) == 1 and outcomes[0].status == "deleted"
    assert not wt.worktree_path.exists()
    assert f"agentnave/{wt.id}" not in _git(
        repo, "for-each-ref", "--format=%(refname:short)", "refs/heads/"
    )
    assert dirs.store.get(wt.id) is None

    # 2. Interrupted `creating` row: clean half-created worktree/branch rolls back; modified one turns needs_attention.
    wt_clean = dirs.create(repo, workspace_id="ws-1")
    wt_clean.lease.release()
    rec_clean = dirs.store.get(wt_clean.id)
    assert rec_clean is not None
    dirs.store.delete(wt_clean.id)
    dirs.store.insert_creating(replace(rec_clean, state="creating", phase="copying_include"))
    out_clean = dirs.recover()
    assert len(out_clean) == 1 and out_clean[0].status == "rolled_back"
    assert not wt_clean.worktree_path.exists()
    assert dirs.store.get(wt_clean.id) is None

    wt_dirty = dirs.create(repo, workspace_id="ws-1")
    wt_dirty.lease.release()
    (wt_dirty.worktree_path / "README.md").write_text("user touched\n", encoding="utf-8")
    rec_dirty = dirs.store.get(wt_dirty.id)
    assert rec_dirty is not None
    dirs.store.delete(wt_dirty.id)
    dirs.store.insert_creating(replace(rec_dirty, state="creating", phase="copying_include"))
    out_dirty = dirs.recover()
    assert len(out_dirty) == 1 and out_dirty[0].status == "needs_attention"
    assert wt_dirty.worktree_path.is_dir()
    saved_dirty = dirs.store.get(wt_dirty.id)
    assert saved_dirty is not None and saved_dirty.state == "needs_attention"
    dirs.close()
