# 2026-06-09 — Debug and Fix getAttribute Uncaught TypeError

- Date: 2026-06-09
- GitHub Issue: #257
- Status: Draft

## Goal
Identify the JavaScript object that lacks the `getAttribute` function causing the `Uncaught TypeError: t.getAttribute is not a function` crash on Naver, and resolve the crash by implementing the proper stubs or fallback.

## Non-goals
- Adding permanent `Object.prototype` modifications.
- Implementing fully-featured DOM APIs beyond the necessary stubs to prevent crashes.

## Context / Constraints
- The crash happens at column 46058 of `preload.20fd9b94.js` inside Sizzle's `ot.attr(t, e)` function: `t.getAttribute(e)`.
- We need to find the runtime class/constructor of the object `t`.
- We must follow the strict developer workflow: Plan -> Review -> Act -> Verify.

## Approach (Checklist)
- [ ] **Step 0: Recon**
  - Verify the crash log is clean and points to column 46058.
- [ ] **Step 1: Implementation (Temporary Debug)**
  - Define a temporary `Object.prototype.getAttribute` debug wrapper in `src/js_bootstrap.js` that logs `this` constructor/type and returns `null`.
  - Run the daemon and navigate to `https://www.naver.com`.
  - Check the daemon log to inspect what type of object triggered the debug probe.
- [ ] **Step 2: Permanent Fix**
  - Remove the temporary `Object.prototype` modification.
  - Apply the proper stub (e.g., to `window` or `Location` or the corresponding object class) in `src/js_bootstrap.js`.
- [ ] **Step 3: Verification & Tests**
  - Run `cargo test --lib -- --test-threads=1` to ensure no regressions.
  - Run the daemon again, navigate to Naver, take a screenshot, and verify the console is clean of the `getAttribute` exception.

## Validation
- **Commands to run:**
  - `cargo test --lib -- --test-threads=1`
  - Run the daemon and fetch/tick Naver to see logs.
- **Expected output:**
  - No `getAttribute` TypeError.
  - Homepage should load further/completely without standard bundle exceptions.

## Risks & Rollback
- **Risks:** Temporary debug properties on `Object.prototype` can cause side-effects. We will remove it immediately after gathering logs.
- **Rollback steps:** `git restore src/js_bootstrap.js`.

## Open Questions
- None.
