# Release Process

Follow this checklist whenever the user explicitly asks to release a new version of AgentNave, install/verify the local official app before release, or recover a failed release workflow.

## Scope

- A release request is complete only after the version PR is reviewed and merged into `origin/main`, the `.github/workflows/python.yml` release pipeline finishes green, and the corresponding tag and GitHub Release are published with all artifacts. Follow any narrower scope the user explicitly states.
- Never manually create or move a release tag, and never publish a GitHub Release manually bypassing CI.

## Pre-release local app parity

- Local official installation (`/Applications/AgentNave.app` and its embedded service) must only contain reviewed code already merged into `origin/main` (unmerged work uses `scripts/preview_tray.sh`).
- Releases may lag behind local installation, but the released commit must match the code running in the local official app (excluding version metadata). Before cutting or completing a release, verify parity between merged `main` and the local installation; rebuild and reinstall if they differ.

## Version PR checklist

Update all of the following in sync inside one version PR:

1. **Version numbers** (strictly increasing stable `MAJOR.MINOR.PATCH`):
   - `pyproject.toml` (`project.version`)
   - `src/agentnave/__init__.py` (`__version__`)
   - `apps/tray/Cargo.toml` (`package.version`)
2. **Lockfiles**:
   - `uv.lock` (`uv lock`)
   - `apps/tray/Cargo.lock`
3. **Documentation**:
   - `docs/installation.md`: update version examples and replace the single current-version upgrade note (previous upgrade notes remain in `docs/releases/`).
   - `README.md` and `README.en.md`: update paired version examples.
   - `docs/releases/v<version>.md`: non-empty release notes with user-facing changes and a `## 升级` section (CI strips everything from `## 升级` onward when building the in-app updater manifest).

## CI automated release pipeline

When a version bump merges to `origin/main`, `.github/workflows/python.yml` runs automatically (ordinary merges without a version bump do not release):

1. `release-metadata` verifies `pyproject.toml` vs `agentnave.__version__`, strictly increasing semver, paired skill files in the source archive, and non-empty `docs/releases/v<version>.md`.
2. `verify` runs formatting, lint, type checks, tests, `uv build`, and installed wheel verification on `ubuntu-latest`, `macos-latest`, and `windows-latest`.
3. `release` creates tag `v<version>` and a **draft** GitHub Release, then uploads the Python wheel.
4. `tray-release` (`macos-latest`) builds the signed universal macOS `.dmg` and `.app.tar.gz` (requires repo secret `TAURI_SIGNING_PRIVATE_KEY`) and uploads them with `latest.json`.
5. `windows-release` (`windows-latest`) builds the signed NSIS `_x64-setup.exe`, uploads it, and adds `windows-x86_64` to `latest.json`.
6. `publish-release` publishes the draft Release once every asset is attached, marking `Latest` only when `v<version>` is the highest semver release.

## Failure recovery

- **Transient or external failure** (runner/network flake, secret restored): rerun the original failed workflow on the same commit. The `release` job safely resumes an existing draft tag/release; never move an existing tag.
- **Code or release-metadata fix needed**: open a new PR with another version increment. A normal push with the same version does not retry a prior release.
