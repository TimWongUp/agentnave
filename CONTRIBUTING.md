# Contributing to AgentNave

Contributions are welcome through GitHub Issues and pull requests. Keep changes focused on
AgentNave's role as a local STDIO MCP adapter for CLI-based subagents; planning, orchestration,
review, retries, permissions, and worktree management belong to the calling Agent Manager.

## Report an issue

Search existing Issues before opening a new one. For a bug, include the operating system, Python
version, provider CLI and version when relevant, a minimal reproduction, and the expected and
actual behavior. Redact credentials, provider output, local paths, and other private data.

Do not report security vulnerabilities in a public Issue. Follow [SECURITY.md](SECURITY.md)
instead.

## Set up the development environment

AgentNave requires Python 3.12 or later, `uv`, and macOS or Linux.

```bash
git clone https://github.com/TimWongUp/agentnave.git
cd agentnave
uv sync --locked --all-groups
```

## Make a change

- Keep the change limited to one coherent behavior or documentation concern.
- Preserve provider-native defaults unless the caller explicitly supplies an allowlisted option.
- Add or update the smallest relevant test when behavior changes and existing coverage would not
  catch a regression.
- Automated tests must not require authenticated provider CLIs or consume provider quota. Follow
  the repository's existing fake-executable patterns instead.

Run the project checks before opening a pull request:

```bash
uv run ruff format --check .
uv run ruff check .
uv run pyright
uv run pytest
```

In the pull request, explain the user-visible change, the validation performed, and any remaining
limitations or platform-specific behavior.

## Version releases

Prepare a release in a pull request: increase the stable `MAJOR.MINOR.PATCH` version in
`pyproject.toml` and `src/agentnave/__init__.py`, refresh `uv.lock` with `uv lock`, update the install
examples, and add `docs/releases/v<version>.md` with user-facing changes and migration instructions.
The CI metadata check rejects inconsistent versions, decreases, and missing release notes.

After the version change is merged into `origin/main`, the existing Python workflow waits for
both macOS and Linux verification jobs, then creates the version tag and GitHub Release at that
exact push commit. Ordinary changes without a version bump do not publish. The Git installation
channel remains canonical; this workflow does not publish to PyPI or upload binary distributions.

After a transient or external failure is resolved, rerun the original workflow. If recovery requires
code or release metadata changes, submit a new pull request with another version increase: reruns
use the original commit, and an ordinary same-version push does not retry an earlier release.
A published release at the same tag and commit is a no-op; a conflicting tag, missing tag for an
existing release, or existing draft requires manual review and is never overwritten. Confirm that
the release job and GitHub Release have completed before reporting a version as published.
