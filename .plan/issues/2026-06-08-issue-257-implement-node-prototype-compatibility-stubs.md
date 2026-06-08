# 2026-06-08 — Implement Node Prototype Compatibility Stubs

- Date: 2026-06-08
- GitHub Issue: #257
- Status: Planning

## Goal

Resolve the remaining `TypeError: t.getAttribute is not a function` and other potential element-method crashes on Naver.com by defining safe, fallback element-like methods on `Node.prototype` in `src/js_bootstrap.js`. This ensures that all non-element DOM nodes (such as `TextNode`, `Comment`, `DocumentFragment`, and `DocumentType`) can safely handle these calls without crashing the JS runtime.

## Non-goals

- Implementing full element behavior on non-element nodes.

## Context / Constraints

- Ad SDKs and React libraries traverse the DOM tree (e.g. `n = n.parentElement || n.parentNode`) and call methods like `getAttribute`, `closest`, `matches`, or `querySelector` on the resolved nodes.
- If the traversal reaches a `TextNode`, `Comment`, or `DocumentFragment` (which lack these methods), the JS runtime throws a fatal `TypeError`.
- Defining these methods on `Node.prototype` allows `Element` to override them, while providing safe fallback defaults (`null`, `false`, or empty collections) for all other node types.

## Approach (Checklist)

- [ ] **Step 1: Add element compatibility methods to `Node.prototype` in `src/js_bootstrap.js`**
  - Define `getAttribute: function(name) { return null; }`
  - Define `setAttribute: function(name, value) {}`
  - Define `removeAttribute: function(name) {}`
  - Define `hasAttribute: function(name) { return false; }`
  - Define `closest: function(selector) { return null; }`
  - Define `matches: function(selector) { return false; }`
  - Define `querySelector: function(selector) { return null; }`
  - Define `querySelectorAll: function(selector) { return new NodeList([]); }`
- [ ] **Step 2: Add unit tests in `src/js.rs`**
  - Add a unit test to verify that calling element-like methods (`getAttribute`, `closest`, `matches`, `querySelector`, `querySelectorAll`) on `TextNode`, `Comment`, and `DocumentFragment` returns correct safe defaults without throwing exceptions.
- [ ] **Step 3: Verification**
  - Run `cargo test --lib` to ensure all tests pass.
  - Restart the browser daemon, navigate to Naver, wait 20 ticks, and verify that the page renders without console errors and successfully mounts.

## Validation

- **Commands to run:**
  - `cargo check`
  - `cargo test --lib`
- **Expected output:**
  - Build compiles, and tests pass.
  - Naver screenshot renders successfully with dynamic React elements mounted.

## Risks & Rollback

- **Risks:** Extremely low, as it only provides fallback stubs on non-element nodes.
- **Rollback steps:** Revert changes in `src/js_bootstrap.js`.

## Open Questions

None.
