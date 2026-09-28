# 2026-09-27 — Build Linux browser archive on published release

- Date: 2026-09-27
- GitHub Issue: #291
- Status: Implemented; awaiting pull request review and hosted runner verification

## Goal
Build the GUI browser, browser-daemon, and browser-cli on Linux x86_64 when a GitHub Release is published and attach one archive to that release. Validate the same packaging on pull requests.

## Non-goals
No release is published by this change. No Windows/macOS binaries, installer, runtime changes, or edits to the dirty `feat/258` worktree.

## Context / Constraints
The repository has no existing GitHub Actions workflow or releases. `Cargo.lock` is tracked and `Cargo.toml` defines the daemon and CLI binaries plus the GUI binary at `src/main.rs`. Native GUI dependencies need a Linux runner. The source and dependency build on pull requests must run without a write-capable GitHub token.

## Approach (Checklist)
- [x] **Step 0: Recon** — Read project instructions, Cargo manifest, README, existing workflows/releases, and GitHub issue list. Work on `ci/291` in a separate worktree from `origin/main`.
- [x] **Step 1: Implementation** — Add `.github/workflows/release.yml` with pull request and `release.published` triggers. Checkout the release tag commit for release events; pull requests checkout the proposed merge commit. Grant `contents: read` to the PR-capable build job; install native Linux dependencies and run `cargo build --release --locked --bins`. Create `browser-linux-x86_64.tar.gz` with exactly three top-level executable entries: `browser`, `browser-daemon`, and `browser-cli`, from `target/release/`. Upload it as a workflow artifact named `browser-linux-x86_64`. A separate release-only job downloads the artifact, receives `contents: write`, and attaches the archive to `github.event.release.tag_name` with `gh release upload --clobber` so reruns replace the same named asset. Do not run source code or dependency build steps in the privileged job.
- [ ] **Step 2: Tests** — Parse workflow YAML, lint with actionlint if available, confirm triggers/permissions/jobs by assertions, perform local release binary build and inspect archive when resources allow, and verify pull request CI. Do not publish a throwaway release just to test the upload.
- [ ] **Step 3: Rollout / Rollback** — Review diff; push branch and open pull request referencing #291. First actual release verifies upload. Revert workflow commit to disable the trigger.

## Validation
- **Commands to run:** `cargo build --release --locked --bins`; `tar -tzf browser-linux-x86_64.tar.gz`; YAML parse and actionlint/static assertions; `gh pr checks` after opening pull request.
- **Expected output:** Three binaries exist in archive; pull request job passes; release publish job is skipped on pull requests. Release upload is untested until an actual release is published.

## Risks & Rollback
- **Risks:** Linux native system libraries may differ on hosted runners; V8 prebuilt binary download may be rate-limited; GitHub Actions does not run on local host, so static validation cannot prove hosted build. Release job must never receive untrusted pull request code with a write-capable token.
- **Rollback steps:** Revert the workflow addition; existing build remains unchanged.

## Open Questions
- Runtime system library compatibility on other Linux distributions is not guaranteed; this archive targets the GitHub-hosted Ubuntu Linux x86_64 environment.
