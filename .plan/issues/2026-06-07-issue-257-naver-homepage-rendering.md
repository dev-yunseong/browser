# 2026-06-07 — Naver homepage rendering and dynamic module script support

- Date: 2026-06-07
- GitHub Issue: #257
- Status: Planning

## Goal

Resolve the issue where `https://www.naver.com` renders as a blank white page. Enable visual rendering of the Naver homepage by supporting dynamic module scripts (`<script type="module">` dynamically inserted into the DOM) and resolving their dependencies.

## Non-goals

- Support for all missing HTML5/DOM APIs.
- Perfect pixel-by-pixel match against Chromium for every single sub-element (the target is visual correctness of the main page shell/layout).
- Overhauling the V8 module actor integration.

## Context / Constraints

- The current branch is `fix/279` (where we already checkout from PR #282).
- Dynamic module script tags dynamically appended to the DOM are currently blocked in `js_bootstrap.js` with the message: `Module scripts are not supported yet; dynamic module script signaled error.`.
- In `src/js.rs`, dynamic imports (`host_import_module_dynamically`) only fetch the root module but do not fetch its recursive dependencies, which will fail if a dynamically imported module has static `import` declarations.
- Visual rendering should be validated using screenshots.

## Approach (Checklist)

- [x] **Step 0: Helper for recursive dependency fetching**
  - Implement `resolve_and_fetch_module_dependencies_rec` in `src/js.rs` to recursively fetch the source code of a module's dependencies and store them in `MODULE_SOURCES`.
- [x] **Step 1: Implement JS Host callback for dynamic modules**
  - Register a host function callback `__aura_execute_module_script_in_host(url, code)` in V8.
  - Implement this callback to:
    - Save the root module's source in `MODULE_SOURCES`.
    - Recursively resolve and fetch all dependency sources using `resolve_and_fetch_module_dependencies_rec`.
    - Compile, instantiate, and evaluate the root module.
- [x] **Step 2: Update dynamic import handler to support nested dependencies**
  - Update `host_import_module_dynamically` in `src/js.rs` to call `resolve_and_fetch_module_dependencies_rec` to fetch dependencies of dynamically imported modules before compilation and instantiation.
- [x] **Step 3: Update `js_bootstrap.js` to handle module scripts**
  - Modify `__aura_maybe_run_script` in `src/js_bootstrap.js` to accept `module` script type.
  - Implement `__aura_execute_module_script(script, url, code)` which calls `__aura_execute_module_script_in_host` and fires `load`/`error` events on the script node.
- [ ] **Step 3.5: Fix CLI localhost connection issue**
  - Update `localhost` to `127.0.0.1` in `src/bin/browser_cli.rs` (line 127 and tests) to avoid connection failure when IPv6 resolution takes priority.
- [ ] **Step 3.6: Fix pre-existing integration test failure**
  - Update `tests/repro_failures.rs` to filter out whitespace text layout boxes so that `test_interleaved_block_inline_stacking` passes.
- [ ] **Step 4: Verification**
  - Run `cargo test --lib` and verify no regressions.
  - Start the daemon headlessly: `timeout 90s ./target/debug/browser-daemon --no-gui --port 7070`.
  - Navigate to `https://www.naver.com`: `timeout 45s ./target/debug/browser-cli --port 7070 navigate https://www.naver.com`.
  - Take a screenshot: `timeout 10s ./target/debug/browser-cli --port 7070 screenshot /path/to/naver.png`.
  - View the screenshot to verify it renders the Naver shell/layout instead of a blank screen.

## Validation

- Build must compile successfully.
- Library tests must pass.
- Naver homepage rendering screenshot must show the visual shell/layout of Naver instead of a blank screen.

## Risks & Rollback

- **Risks:** Synchronous network fetches on the V8 isolate thread for dynamic scripts/imports could block the daemon longer. However, since the browser daemon is single-threaded for JS/HTML execution, this matches the existing synchronous model for static modules and is safe.
- **Rollback:** Revert changes in `src/js.rs` and `src/js_bootstrap.js` to return to the previous state.
