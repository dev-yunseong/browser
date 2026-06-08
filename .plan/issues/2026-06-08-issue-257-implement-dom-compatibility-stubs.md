# 2026-06-08 — Implement DOM Compatibility Stubs (createElementNS, postMessage)

- Date: 2026-06-08
- GitHub Issue: #257
- Status: Planning

## Goal

Implement additional DOM APIs required to prevent crashes and hangs in Naver's script execution pipeline:
1. Implement `document.createElementNS` and `contentDocument.createElementNS` as safe delegates to `document.createElement`.
2. Ensure iframe `contentWindow` extends `EventTarget` (so it supports `addEventListener`).
3. Implement `postMessage` on both global `window` and iframe `contentWindow` to asynchronously dispatch a `message` event.

## Non-goals

- Perfect implementation of XML/SVG namespaces.
- Full security verification of origins during message passing.

## Context / Constraints

- The current branch is `fix/279`.
- Dynamic scripts fail to initialize React components due to:
  - `TypeError: c.createElementNS is not a function` when creating SVG/icon elements.
  - `TypeError: r.current.contentWindow.postMessage is not a function` when interacting with nested iframes.
- Some libraries use `postMessage` to schedule macro-tasks, meaning a pure no-op stub would cause hangs. Message events must be dispatched asynchronously.

## Approach (Checklist)

- [ ] **Step 1: Implement `createElementNS` in `src/js_bootstrap.js`**
  - Add `createElementNS: function(ns, tag) { return document.createElement(tag); }` on `document`.
  - Add `createElementNS: function(ns, tag) { return document.createElement(tag); }` on iframe's `contentDocument`.
- [ ] **Step 2: Make `contentWindow` an `EventTarget` and add `postMessage` in `src/js_bootstrap.js`**
  - In `HTMLIFrameElement.prototype.contentWindow`, instantiate `this._contentWindow` as `new EventTarget()`.
  - Add a helper function `__aura_post_message_impl(target, message, targetOrigin)` that uses `setTimeout` to dispatch a `message` event on the target object.
  - Add `postMessage` to the iframe's `contentWindow` delegating to `__aura_post_message_impl`.
  - Add `postMessage` on global `window` (globalThis) delegating to `__aura_post_message_impl`.
- [ ] **Step 3: Add unit tests in `src/js.rs`**
  - Verify `document.createElementNS` returns a valid element.
  - Verify global `postMessage` is a function.
  - Verify `contentWindow` is an `EventTarget` and calling `postMessage` asynchronously triggers `message` event listeners.
- [ ] **Step 4: Verification**
  - Run `cargo test --lib`.
  - Restart browser-daemon, navigate to Naver, and verify rendering via screenshot.

## Validation

- **Commands to run:**
  - `cargo check`
  - `cargo test --lib`
- **Expected output:**
  - Build compiles, and tests pass.
  - Naver screenshot renders homepage layout.

## Risks & Rollback

- **Risks:** Asynchronous events introduce macrotask scheduling, but this matches browser specs and is safe.
- **Rollback steps:** Revert changes in `src/js_bootstrap.js`.

## Open Questions

None.
