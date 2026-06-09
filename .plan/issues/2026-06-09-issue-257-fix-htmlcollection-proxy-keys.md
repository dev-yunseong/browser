# 2026-06-09 — Fix HTMLCollection and StyleSheetList Proxy Keys

- Date: 2026-06-09
- GitHub Issue: #257
- Status: Draft

## Goal
Fix `HTMLCollection` and `StyleSheetList` Proxies so that they only expose numeric indices and standard non-enumerable properties, preventing Sizzle/jQuery from resolving internal properties (like `_resolver`) as DOM elements and crashing.

## Non-goals
- Over-complicating Proxy traps for unrelated classes.

## Context / Constraints
- We observed that Sizzle/jQuery accesses `getAttribute` on `Function` objects (such as `_resolver`) when mapping/iterating collections.
- This happens because `_resolver` is an own property on the Proxy target and is enumerable by default, making it visible to key iterations (`Object.keys`, `for...in`).
- We need to:
  1. Hide `_resolver` by making it non-enumerable.
  2. Implement `ownKeys` and `getOwnPropertyDescriptor` traps in `HTMLCollection` and `StyleSheetList` Proxies to correctly report only numeric indices and hide internal properties.
  3. Remove the temporary `Object.prototype` debug hooks.

## Approach (Checklist)
- [ ] **Step 1: Hide `_resolver` & Implement Proxy traps**
  - Update `HTMLCollection` constructor:
    - Use `Object.defineProperty` to define `_resolver` as non-enumerable.
    - Implement `ownKeys` trap to return `["0", "1", ..., "length"]`.
    - Implement `getOwnPropertyDescriptor` trap to return descriptor for indices and `length`.
  - Update `StyleSheetList` constructor similarly.
  - Remove temporary `Object.prototype.getAttribute` and `Object.prototype.removeAttribute` debug hooks.
- [ ] **Step 2: Build & Verification**
  - Run `cargo build` to compile the bootstrap changes.
  - Restart the browser daemon.
  - Navigate to Naver and run `tick 20` to verify that the page loads without any TypeError.
- [ ] **Step 3: Run Tests**
  - Run `cargo test --lib -- --test-threads=1` to ensure all tests pass.

## Validation
- **Commands to run:**
  - `cargo test --lib -- --test-threads=1`
  - Re-run daemon navigation and check output.
- **Expected output:**
  - `cargo test` passes.
  - Sizzle exception is gone and page renders successfully.

## Risks & Rollback
- **Risks:** Adding `ownKeys` and `getOwnPropertyDescriptor` could affect JS code that iterates over CSS stylesheets or DOM elements, but returning standard descriptors is the correct way.
- **Rollback steps:** `git restore src/js_bootstrap.js`.
