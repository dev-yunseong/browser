# 2026-06-08 — Implement HTMLAnchorElement URL Properties

- Date: 2026-06-08
- GitHub Issue: #257
- Status: Planning

## Goal

Provide full DOM compatibility for `HTMLAnchorElement` URL-like properties to prevent modern web application bundles and polyfills (such as axios/URL utilities) from crashing with `TypeError: Cannot read properties of undefined (reading 'charAt')` or similar errors on Naver.com.

Specifically, we will implement the following getters and setters on `HTMLAnchorElement` in `src/js_bootstrap.js`:
- `protocol`
- `host`
- `hostname`
- `port`
- `pathname`
- `search`
- `hash`
- `origin` (getter only)

## Non-goals

- Perfect pixel-by-pixel match against Chromium.
- Implementing non-standard anchor properties.

## Context / Constraints

- The current branch is `fix/279`.
- The JS runtime is V8 via `rusty_v8`.
- The `URL` constructor is already fully implemented and available in `src/js_bootstrap.js`. We can delegate to it safely.
- In browsers, getters return the absolute resolved URL relative to the document base URL. Setting any property updates the underlying `href` attribute.

## Approach (Checklist)

- [ ] **Step 1: Implement HTMLAnchorElement URL Properties**
  - Modify `HTMLAnchorElement` in `src/js_bootstrap.js` to add getters/setters for `protocol`, `host`, `hostname`, `port`, `pathname`, `search`, `hash`, and getter for `origin`.
  - Use a private helper method (e.g. `_getURL()` and `_setURLProp()`) to create a `URL` object with fallback base URL `location.href || document.baseURI || undefined`, modify it, and update the `href` attribute.
- [ ] **Step 2: Add JS tests for HTMLAnchorElement URL properties**
  - Add a new integration test or unit test in the test suite to verify that anchor URL properties are resolved and updated correctly.
- [ ] **Step 3: Build & run existing library tests**
  - Run `cargo check` and `cargo test --lib` (using `-- --test-threads=1` to avoid state contamination) to verify no regressions.
- [ ] **Step 4: Verify Naver Navigation & Screenshot**
  - Start/ensure browser-daemon is running.
  - Navigate to `https://www.naver.com` and tick.
  - Take a screenshot and visually verify that the Naver homepage layout/shell is rendering rather than a blank white page.

## Validation

- **Commands to run:**
  - `cargo check`
  - `cargo test --lib`
  - Navigate and tick:
    ```bash
    ./target/debug/browser-cli --port 7070 navigate https://www.naver.com
    ./target/debug/browser-cli --port 7070 tick 10
    ```
  - Take a screenshot:
    ```bash
    ./target/debug/browser-cli --port 7070 screenshot /home/yunseong/.gemini/antigravity-cli/brain/d2a91a85-f6c6-40c8-8fd2-4993f019f1cb/naver_current.png
    ```
- **Expected output:**
  - Build compiles, and tests pass.
  - Naver screenshot is no longer a blank white screen.

## Risks & Rollback

- **Risks:**
  - Potential performance overhead of constructing `new URL` on every property access. However, these properties are accessed on demand, and this matches standard behavior, so the overhead is negligible.
- **Rollback steps:**
  - Revert the changes in `src/js_bootstrap.js`.

## Open Questions

None.
