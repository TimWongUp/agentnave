"""The permissions `codex exec` would run a turn with, resolved from `config/read` (ADR-0120).

`thread/resume` restores the approval policy, reviewer, permission profile and workspace roots a
thread last ran with, and a kept process keeps whatever its previous turn set. So the bridge
sends every one of them explicitly, on every thread and turn request, resolved the way Codex
0.171 resolves a fresh exec run (codex-rs/exec/src/lib.rs, core/src/config/mod.rs and
config/src/config_toml.rs): from the effective config the app-server reads from disk for this
cwd, after the `--config` overrides it was started with.
"""

from __future__ import annotations

import os
import subprocess
from dataclasses import dataclass
from typing import Any, Literal, cast

# "headless": exec's default, never ask unless the reviewer is auto review;
# "never": `--dangerously-bypass-approvals-and-sandbox`, which keeps never even then.
type ApprovalMode = Literal["headless", "never"]
APPROVAL_MODES: tuple[ApprovalMode, ...] = ("headless", "never")
_AUTO_REVIEWERS = frozenset({"auto_review", "guardian_subagent"})
_TRUST_DECISIONS = frozenset({"trusted", "untrusted"})


def _object(value: object) -> dict[str, Any]:
    return cast(dict[str, Any], value) if isinstance(value, dict) else {}


@dataclass(frozen=True, slots=True)
class Permissions:
    approval_policy: object
    approvals_reviewer: str
    # A legacy sandbox mode, or else a named permission profile.
    sandbox: str | None
    profile: str | None
    workspace_write: dict[str, Any]
    workspace_roots: tuple[str, ...]

    def thread_params(self) -> dict[str, Any]:
        """For thread/start and thread/resume."""
        params = self._common()
        if self.profile is not None:
            params["permissions"] = self.profile
        else:
            params["sandbox"] = self.sandbox
        return params

    def turn_params(self) -> dict[str, Any]:
        """For turn/start, which takes a sandbox policy instead of a mode."""
        params = self._common()
        if self.profile is not None:
            params["permissions"] = self.profile
        else:
            params["sandboxPolicy"] = self._sandbox_policy()
        return params

    def _common(self) -> dict[str, Any]:
        return {
            "approvalPolicy": self.approval_policy,
            "approvalsReviewer": self.approvals_reviewer,
            "runtimeWorkspaceRoots": list(self.workspace_roots),
        }

    def _sandbox_policy(self) -> dict[str, Any]:
        if self.sandbox == "danger-full-access":
            return {"type": "dangerFullAccess"}
        if self.sandbox == "workspace-write":
            settings = self.workspace_write
            return {
                "type": "workspaceWrite",
                "writableRoots": list(_absolute_roots(settings.get("writable_roots"))),
                "networkAccess": settings.get("network_access") is True,
                "excludeTmpdirEnvVar": settings.get("exclude_tmpdir_env_var") is True,
                "excludeSlashTmp": settings.get("exclude_slash_tmp") is True,
            }
        return {"type": "readOnly"}


def _absolute_roots(value: object) -> tuple[str, ...]:
    if not isinstance(value, list):
        return ()
    roots = (
        os.path.expanduser(item) for item in cast(list[object], value) if isinstance(item, str)
    )
    return tuple(root for root in roots if os.path.isabs(root))


def trust_root(cwd: str) -> str | None:
    """The repository Codex looks up trust for: a worktree's main checkout, like Codex."""
    try:
        found = subprocess.run(
            ["git", "-C", cwd, "rev-parse", "--path-format=absolute", "--git-common-dir"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    common = found.stdout.strip()
    if found.returncode != 0 or os.path.basename(common) != ".git":
        return None
    return os.path.dirname(common)


def _trust_level(config: dict[str, Any], cwd: str, root: str | None) -> str | None:
    """The trust decision for cwd, else its repository: exact project keys, as Codex matches."""
    projects = _object(config.get("projects"))

    def fold(key: str) -> str:
        return key.lower() if os.name == "nt" else key

    for path in (cwd, root):
        if path is None:
            continue
        for key in dict.fromkeys((os.path.realpath(path), path)):
            for name, project in projects.items():
                if fold(name) == fold(key):
                    level = _object(project).get("trust_level")
                    return level if isinstance(level, str) else None
    return None


def resolve(config: dict[str, Any], cwd: str, mode: ApprovalMode) -> Permissions:
    """Permissions for a turn in cwd, as a new `codex exec` there would get them."""
    trust: list[str | None] = []

    def trust_decision() -> str | None:
        if not trust:
            trust.append(_trust_level(config, cwd, trust_root(cwd)))
        return trust[0]

    reviewer = config.get("approvals_reviewer")
    reviewer = reviewer if isinstance(reviewer, str) else "user"
    if mode == "never" or (mode == "headless" and reviewer not in _AUTO_REVIEWERS):
        approval: object = "never"
    elif config.get("approval_policy") is not None:
        approval = config["approval_policy"]
    else:
        approval = "untrusted" if trust_decision() == "untrusted" else "on-request"
    workspace_write = _object(config.get("sandbox_workspace_write"))
    sandbox = config.get("sandbox_mode")
    profile = config.get("default_permissions")
    if isinstance(sandbox, str) or not isinstance(profile, str):
        profile = None
        if not isinstance(sandbox, str):
            # No sandbox configured: a directory with a trust decision may write.
            sandbox = "workspace-write" if trust_decision() in _TRUST_DECISIONS else "read-only"
        roots = (cwd, *_absolute_roots(workspace_write.get("writable_roots")))
    else:
        roots = (cwd,)
    return Permissions(
        approval,
        reviewer,
        sandbox,
        profile,
        workspace_write,
        tuple(dict.fromkeys(roots)),
    )
