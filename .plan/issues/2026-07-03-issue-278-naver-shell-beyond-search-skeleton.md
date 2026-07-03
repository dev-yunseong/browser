# 2026-07-03 — Naver shell beyond search skeleton

- Date: 2026-07-03
- GitHub Issue: #278
- Status: Implemented

## Goal

Make `https://www.naver.com` render visible main shell content beyond the tiny search/skip-link skeleton by removing the current JavaScript startup blocker reported as `TypeError: Cannot read properties of undefined (reading 'onsubmit')`.

## Non-goals

- Pixel-perfect parity with Chromium/Naver in this PR.
- Full implementation of every browser API listed in #270.
- Large layout, style, renderer, or module-loader rewrites unless a live blocker proves they are required.

## Context / Constraints

- Issue #278 is part of #257 and #252.
- Current Naver HTML contains `<form id="sform" name="search">`.
- Existing DOM support includes `HTMLFormElement`, `onsubmit` handler properties, `document.forms`, and collection named lookup.
- Browser-compat legacy named access such as `document.search` / `window.search` is not currently implemented. Naver or one of its external bundles may read `document.search.onsubmit` or `window.search.onsubmit`, which would match the current error shape.
- Keep unrelated local files (`storage.json`, `.codex/`, `.claude/worktrees/`) out of the PR.

## Approach (Checklist)

- [x] **Step 0: Recon** (Inspect existing code, locate files)
  - Reproduce or inspect the Naver blocker with daemon/CLI logs where practical.
  - Confirm local DOM behavior for `document.search`, `document.forms.search`, and `document.getElementById('sform')`.
  - Confirm whether `window.search` is also absent.
  - Inspect `src/js_bootstrap.js` collection, document, and event-handler behavior.
- [x] **Step 1: Implementation** (Code changes, file paths)
  - Add focused browser-compatible named document property access for elements by `id`/`name` when the property is not already a real document member.
  - Add matching named-element fallback on `window` only when the property is not already a real global member.
  - Prefer a local `Proxy` around the existing `document` object rather than many per-name definitions, so dynamically parsed or inserted named elements work.
  - Keep `has`, `ownKeys`, and `getOwnPropertyDescriptor` conservative so internal helper properties do not leak into library enumeration and real APIs keep precedence.
- [x] **Step 2: Tests** (Unit tests, manual verification steps)
  - Add JS runtime tests for `document.search.onsubmit`, precedence over built-in document properties, and missing-name behavior.
  - Add JS runtime tests for `window.search` fallback and `in`/`Object.keys` behavior.
  - Run targeted JS tests first, then broader relevant Rust checks.
- [x] **Step 3: Rollout / Rollback** (Feature flags, migration steps)
  - No feature flag or migration needed.
  - If live Naver still fails on a different startup blocker, document it in PR and keep this PR scoped to the named-access fix if tests prove compatibility.

## Validation

- **Commands to run:**
  - `cargo test --lib test_document_named_form_property_supports_onsubmit`
  - `cargo test --lib test_window_named_form_property_supports_onsubmit`
  - `cargo test --lib test_document_named_property_does_not_shadow_existing_member`
  - `cargo test --lib`
  - `cargo build --bins`
  - `timeout 120s ./target/debug/browser-daemon --no-gui --port 7071`
  - `timeout 60s ./target/debug/browser-cli --port 7071 navigate https://www.naver.com`
  - `timeout 30s ./target/debug/browser-cli --port 7071 tick 3`
  - `timeout 30s ./target/debug/browser-cli --port 7071 logs`
  - `timeout 30s ./target/debug/browser-cli --port 7071 screenshot /tmp/naver-278.png`
  - `browser-cli-reviewer`
- **Expected output:**
  - Targeted tests pass.
  - Full relevant build/tests pass.
  - Naver no longer reports the `onsubmit` startup blocker.
  - Screenshot shows more visible shell content than the prior tiny search-only state, or any remaining blocker is captured for the next issue.

## Risks & Rollback

- **Risks:**
  - Named document properties can shadow real document APIs if precedence is wrong.
  - Proxy behavior can affect `in`, `Object.keys`, or library feature detection.
  - Naver may have a second blocker immediately after the `onsubmit` issue.
- **Rollback steps:** `git revert` the commit or remove the document named-property proxy and associated tests.

## Open Questions

- Is the live `onsubmit` error definitely from `document.search.onsubmit`, or from another object access in an external bundle?
- Does Naver require matching named access on `window` in addition to `document` for the next blocker?

## Plan Review Notes

- Fast review: fixed ambiguity around `window.search` vs `document.search` and explicit trap behavior.
- Medium review: kept scope local to DOM named access; no new Rust abstraction or layout work.
- Heavy review: plan is implementable if live Naver reveals the same blocker; any next blocker stays follow-up unless required for this fix verification.

## Implementation Notes

- Added document named-property fallback for form `id`/`name` access.
- Added initial window named-property getters for parsed form `id`/`name` values.
- Verified `document.search.onsubmit` and `window.search.onsubmit` compatibility with unit tests.
- Live Naver verification now shows populated main shell/feed/login/footer content; remaining visual drift is layout/CSS follow-up work.
