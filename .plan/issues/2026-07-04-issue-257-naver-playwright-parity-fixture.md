# 2026-07-04 — Naver Playwright parity fixture

- Date: 2026-07-04
- GitHub Issue: #257
- Status: Implemented, awaiting review

## Goal

Make `https://www.naver.com` verifiable against a Playwright/Chromium baseline at the project viewport, and ensure the browser-daemon output reaches the populated Naver shell without critical startup JS blockers.

## Non-goals

- Pixel-perfect Chromium parity in one PR.
- Broad layout/CSS rewrites that are not required to satisfy #257 acceptance.
- Fixing yunseong.dev parity (#258).

## Context / Constraints

- #278 is closed and removed the `onsubmit` startup blocker.
- #257 acceptance requires reproducible baseline/browser screenshots, populated Naver shell, no critical JS startup blocker, reviewer coverage or documented dedicated command, and `cargo check --all-targets` plus relevant tests.
- Render width is fixed to `800px` unless changed in code.
- Long daemon/browser checks must use `timeout`.
- Keep unrelated dirty files out of this PR.

## Approach (Checklist)

- [x] **Step 0: Recon**
  - Inspected existing CLI/reviewer/docs patterns.
  - Captured current Playwright/Chromium baseline for Naver at 800px.
  - Captured current browser-daemon Naver screenshot at the same width.
  - Compared images and identified remaining layout/CSS drift as follow-up scope.
- [x] **Step 1: Implementation**
  - Added a reproducible documented command for Naver Playwright baseline capture.
  - Added documented browser-daemon navigation, log, and screenshot capture for the same URL/viewport.
  - Updated `browser-cli-reviewer` with a dedicated Naver parity verification path.
  - No runtime/layout code change was needed for #257 startup/shell acceptance because the current browser output already populates the main Naver shell and logs show no critical startup blocker.
- [x] **Step 2: Tests**
  - No targeted Rust tests were required because this change only updates verification docs/reviewer instructions.
  - Ran `cargo check --all-targets`.
  - Ran Naver baseline/browser capture commands and inspected screenshots.
- [x] **Step 3: Follow-up Tracking**
  - Created #288 for remaining layout/CSS visual drift.
  - Remaining risks should be recorded in the PR body.

## Validation

- **Ran:**
  - `cargo check --all-targets` — passed with existing warnings.
  - Playwright baseline capture command for `https://www.naver.com` at 800px — generated `/tmp/browser-naver-parity/naver-playwright.png` (800 x 1200).
  - Browser-daemon navigation/log/screenshot commands for `https://www.naver.com` — generated `/tmp/browser-naver-parity/naver-browser.png` (800 x 2251).
  - `browser-cli logs` after navigation — no critical startup blocker such as `Cannot read properties of undefined (reading 'onsubmit')`.
- **Observed output:**
  - Browser text output includes populated Naver shell content, including search/autocomplete, pay/notification/cart, newsstand, feed tabs, article/content items, login, and footer sections.
  - Browser screenshot is populated beyond raw placeholders.
  - Browser screenshot is not visually identical to Chromium; substantial layout/CSS drift remains.
  - Follow-up issue #288 tracks remaining Naver visual drift.

## Risks & Rollback

- **Risks:**
  - Live Naver content changes frequently, making image comparison noisy.
  - External resources may intermittently fail.
  - Full visual parity may require several layout/CSS issues beyond #257 scope.
- **Rollback steps:** Revert script/docs/code changes for the fixture path.

## Open Questions

- Generated screenshots are not committed; reproducible commands and local output paths are documented.
- Naver stays as a dedicated heavier verification path in `browser-cli-reviewer`.
