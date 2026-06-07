# 2026-06-07 — Prevent Naver daemon hang

- Date: 2026-06-07
- GitHub Issue: #279
- Status: Implemented

## Goal

Make `https://www.naver.com` debugging bounded and observable. After Naver load starts, daemon control endpoints must not hang indefinitely. `status`, `tick`, `logs`, and `screenshot` should return either useful data or a clear busy/error response within command timeouts.

## Non-goals

- Full Naver visual parity.
- Complete implementation of every missing browser API.
- Large JS runtime migration or actor architecture rewrite.
- Pixel comparison against Playwright beyond smoke validation.

## Context / Constraints

- Current branch: `fix/279`.
- Naver initial pipeline begins: CSS fetch, DOM/style/layout/render logs appear.
- After initial work, the daemon can become unreliable:
  - `browser-cli --port 7070 tick 1` timed out.
  - `GET /screenshot` timed out.
  - `GET /status` returned an empty reply in one observed run.
- Engine actor is single-threaded, so long JS/module work blocks control commands.
- Existing project rule: wrap long render/integration loops with `timeout`.
- This plan supports #257 and #278 by making the Naver debug loop usable first.

## Approach (Checklist)

- [x] **Step 0: Recon** (Inspect existing code, locate files)
  - Inspect `src/engine.rs` actor send/receive methods for indefinite `recv()` calls.
  - Inspect `src/js.rs` event-loop draining, module evaluation, dynamic import, and fetch paths.
  - Inspect `src/bin/browser_daemon.rs` `/tick`, `/status`, `/page`, and `/screenshot` handlers.
  - Reproduce with bounded commands and capture daemon logs.

- [x] **Step 1: Implementation** (Code changes, file paths)
  - Add bounded reply waits to daemon-facing `EngineHandle` methods where hangs affect control endpoints.
  - Return explicit busy/error HTTP responses when the actor does not answer in time.
  - Add short, consistent timeouts for external classic/module script fetches.
  - Cap per-call JS tick work so one `tick` cannot drain unlimited queued tasks.
  - Preserve existing behavior for normal pages and CLI commands.

- [x] **Step 2: Tests** (Unit tests, manual verification steps)
  - Add focused tests for timeout/busy behavior where practical.
  - Run `cargo build --bins`.
  - Run relevant engine/daemon tests.
  - Manually run Naver smoke validation with `timeout`.

- [x] **Step 3: Rollout / Rollback** (Feature flags, migration steps)
  - No migration or feature flag expected.
  - If behavior regresses normal pages, revert the commit for #279.

## Validation

- **Commands to run:**

```bash
cargo build --bins
cargo test --lib
cargo test --bin browser-daemon
timeout 90s ./target/debug/browser-daemon --no-gui --port 7070
timeout 45s ./target/debug/browser-cli --port 7070 navigate https://www.naver.com
timeout 10s curl -sS http://127.0.0.1:7070/status
timeout 15s ./target/debug/browser-cli --port 7070 tick 1
timeout 10s ./target/debug/browser-cli --port 7070 logs
timeout 10s curl -sS -o /tmp/naver_browser.png http://127.0.0.1:7070/screenshot
```

- **Expected output:**
  - Build/tests pass, warnings acceptable if pre-existing.
  - Naver load may still have console errors or incomplete visuals.
  - No validation command hangs indefinitely.
  - If actor is busy, API returns a clear non-2xx busy/error response instead of blocking.
  - Console logs are available for the next compatibility issue.

## Validation Results

- `cargo test --bin browser-cli --bin browser-daemon`: pass.
- `cargo build --bins`: pass.
- `cargo test --lib`: pass.
- `timeout 45s ./target/debug/browser-cli --port 7070 navigate https://www.naver.com`: pass; returns initial Naver shell.
- `timeout 10s curl -sS -i http://127.0.0.1:7070/status`: pass; returns `200`.
- `timeout 15s ./target/debug/browser-cli --port 7070 tick 1`: pass; returns `worked=true rerendered=true`.
- `timeout 10s ./target/debug/browser-cli --port 7070 logs`: pass; returns console entries.
- `timeout 10s curl -sS -o /tmp/naver_browser.png -w '%{http_code} %{size_download}\n' http://127.0.0.1:7070/screenshot`: pass; returns `200 3372`.
- Visual inspection of `/tmp/naver_browser.png`: non-hanging but still visually failing; output is mostly blank white with tiny text near the top. Full Naver rendering remains follow-up work.

## Risks & Rollback

- **Risks:**
  - Too-short actor timeout could report busy during legitimate slow render.
  - Too-strict JS tick cap could delay page initialization.
  - Script fetch timeout could skip slow but important resources.

- **Rollback steps:** (e.g., `git revert`, toggle flag off)
  - Revert the #279 commit if normal sites regress.
  - Increase timeout constants if validation shows false busy responses.

## Open Questions

- What timeout budget should `/status`, `/tick`, and `/screenshot` use for local CLI ergonomics?
- Should busy responses be HTTP `503` or `202` with a retry hint?
- Should script fetch timeout be shared with existing page/CSS fetch timeout policy?
