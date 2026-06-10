# 2026-06-09 — Fix HTMLCollection Proxy Traps and getAttribute/removeAttribute TypeError

- Date: 2026-06-09
- GitHub Issue: #257
- Status: Draft

## Goal
Resolve the `getAttribute`/`removeAttribute` TypeError by fixing bugs in `HTMLCollection`'s Proxy traps, allowing libraries (like jQuery/Sizzle) to correctly determine if DOM collections are array-like.

## Non-goals
- Over-complicating proxy traps for non-problematic classes.

## Context / Constraints
- We discovered that `0 in HTMLCollection` returned `false` because:
  1. The `has` trap checked `typeof prop === 'string'`, which is false when `in` operator receives a number.
  2. The `has` trap used `target.length`, which is `undefined` on the raw `target` (it should use `target._items().length`).
- This caused jQuery's `isArraylike` test to return `false` for `HTMLCollection` collections (e.g. from `getElementsByTagName`), forcing it to loop via `for...in` and access function properties (`item`, `namedItem`), which got passed to Sizzle matcher and crashed on `getAttribute`/`removeAttribute` calls.

## Approach (Checklist)
- [ ] **Step 0: Cleanup Debug Hook**
  - Restore `src/js.rs` to remove `eprintln!` log statement in `append_console_entry`.
  - Remove `Object.prototype.getAttribute` and `Object.prototype.removeAttribute` debug stubs from `src/js_bootstrap.js`.
- [ ] **Step 1: Implementation**
  - Update `HTMLCollection` constructor's Proxy traps (`get` and `has`) in `src/js_bootstrap.js` to correctly handle both string and number indices, and use `target._items().length` instead of `target.length`.
  - Update `StyleSheetList` constructor's Proxy traps similarly in `src/js_bootstrap.js`.
- [ ] **Step 2: Verification & Tests**
  - Run `cargo test --lib -- --test-threads=1` to check for regressions.
  - Start the daemon, navigate to Naver, tick 20 times, check daemon logs, and verify that the page loads further (e.g. dynamic elements are mounted).
  - Capture a verification screenshot.

## Validation
- **Commands to run:**
  - `cargo test --lib -- --test-threads=1`
  - Re-run daemon navigation and check output.
- **Expected output:**
  - `cargo test` passes.
  - Sizzle exception is gone from logs.
  - Screen displays rendered elements of Naver.

## Risks & Rollback
- **Risks:** Modifications to proxy traps could affect other DOM collections. We will run the entire library test suite.
- **Rollback steps:** `git restore src/js_bootstrap.js src/js.rs`.
