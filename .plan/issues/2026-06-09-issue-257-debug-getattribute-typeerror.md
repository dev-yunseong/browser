# 2026-06-09 — Debug and Fix getAttribute Uncaught TypeError

- Date: 2026-06-09
- GitHub Issue: #257
- Status: Draft

## Goal
Identify the JavaScript object that lacks the `getAttribute` function causing the `Uncaught TypeError: t.getAttribute is not a function` crash on Naver.com, and resolve the crash by implementing the proper stubs.

## Non-goals
- Keep temporary `Object.prototype` modifications in production code.
- Implement fully-featured DOM APIs beyond the necessary stubs to prevent crashes.

## Context / Constraints
- Sizzle or another library raises `<unknown>:46058: Uncaught TypeError: t.getAttribute is not a function`.
- We need to identify what runtime constructor / class `t` has when it is accessed.
- We must follow the strict developer workflow: Plan -> Review -> Act -> Verify.

## Approach (Checklist)
- [ ] **Step 1: Temporary Debug Hook**
  - Define temporary `Object.prototype.getAttribute` and `Object.prototype.removeAttribute` getters in `src/js_bootstrap.js` to print the constructor and keys of the offending object, return a dummy function, and not crash.
  - Run the browser daemon and navigate/tick to trigger the error.
  - Inspect the printed debug log to identify the constructor/prototype.
- [ ] **Step 2: Permanent Fix**
  - Remove the temporary `Object.prototype` hooks.
  - Add `getAttribute`, `setAttribute`, `removeAttribute`, `hasAttribute` stubs to the identified target class/prototype (e.g. `this._contentDocument` inside `HTMLIFrameElement`, or another class).
- [ ] **Step 3: Verification & Tests**
  - Run `cargo test --lib -- --test-threads=1` to check for regressions.
  - Re-run browser daemon, fetch Naver, and verify the console is clean of the `getAttribute` TypeError.

## Validation
- **Commands to run:**
  - `cargo test --lib -- --test-threads=1`
  - Run the daemon and navigation flow.
- **Expected output:**
  - No `getAttribute` TypeError in logs.
  - Dynamic elements render successfully.

## Risks & Rollback
- **Risks:** Temporary traps on `Object.prototype` can cause recursion or unexpected side-effects if not implemented carefully.
- **Rollback steps:** `git restore src/js_bootstrap.js`.
