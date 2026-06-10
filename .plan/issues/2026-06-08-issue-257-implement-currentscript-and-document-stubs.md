# 2026-06-08 — Implement currentScript and Document Attribute Stubs

- Date: 2026-06-08
- GitHub Issue: #257
- Status: Planning

## Goal

Resolve the remaining `TypeError: t.getAttribute is not a function` on Naver.com by:
1. Implementing `document.currentScript` to return the currently executing script element (using a last-parsed script element heuristic).
2. Implementing safe attribute-related stubs (`getAttribute`, `setAttribute`, `hasAttribute`, `removeAttribute`) on the `document` object to prevent type crashes when scripts fall back to `document`.

## Non-goals

- Perfect tracking of async/deferred scripts executing out of parsing order.
- Adding full element capabilities to `document`.

## Context / Constraints

- In `src/js_bootstrap.js`, `document` is defined as a plain JavaScript object. It lacks `currentScript` and element-like attribute methods.
- Minified ad SDKs (such as `gfp-display-sdk.js` or GPT) inspect `document.currentScript` to get configuration attributes and fall back to `document` if it is null or undefined.
- If they call `.getAttribute(...)` on the fallback `document` object, it throws a `TypeError: t.getAttribute is not a function` because `document` is a plain object without these methods.

## Approach (Checklist)

- [ ] **Step 1: Add currentScript getter to `document` in `src/js_bootstrap.js`**
  - Define `get currentScript()` on `document` to return the last script element found via `document.getElementsByTagName('script')` (or `null` if none exist).
- [ ] **Step 2: Add attribute-related stub methods to `document` in `src/js_bootstrap.js`**
  - Implement `getAttribute: function(name) { return null; }`
  - Implement `setAttribute: function(name, value) {}`
  - Implement `removeAttribute: function(name) {}`
  - Implement `hasAttribute: function(name) { return false; }`
- [ ] **Step 3: Add unit tests in `src/js.rs`**
  - Add a unit test to verify that `document.currentScript` returns the script element when evaluated inside a script.
  - Verify that calling `document.getAttribute` returns `null` instead of throwing a TypeError.
- [ ] **Step 4: Verification**
  - Run `cargo test --lib` to ensure all tests pass.
  - Restart the browser daemon, navigate to Naver, wait 20 ticks, and verify that the page renders without `getAttribute` exceptions.

## Validation

- **Commands to run:**
  - `cargo check`
  - `cargo test --lib`
- **Expected output:**
  - Build compiles, and tests pass.
  - Naver screenshot renders successfully with dynamic React elements mounted.

## Risks & Rollback

- **Risks:** Extremely low, as this only adds missing standard properties/methods to `document`.
- **Rollback steps:** Revert changes in `src/js_bootstrap.js`.

## Open Questions

None.
