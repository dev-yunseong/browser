# 2026-06-09 — [DOM] Add observer and ShadowRoot compatibility for modern bundles

- Date: 2026-06-09
- GitHub Issue: #256
- Status: Draft (Revised after Fast+Medium review)

## Goal

Fix Naver (and similar SPA sites) failing to render content by fixing the
`requestAnimationFrame` pipeline and filling minor Observer/ShadowRoot API gaps.

Root causes confirmed:
1. **`navigate()` does not drain the RAF queue** — `raf_cb` (native, js.rs:3183)
   pushes callbacks to `RAF_TASKS`, but `tick_js()` is never called in the
   `navigate()` / `init_js_for_page()` flow. React registers a RAF callback during
   init, but it never fires before `get_document_html()` is called.
2. **`raf_cb` returns no value** (`_rv` ignored) — React stores the RAF handle for
   `cancelAnimationFrame(id)`. An `undefined` return is not a crash but is
   non-conformant; `ric_cb` (sibling function) already returns an id correctly.
3. **`IntersectionObserver` missing properties** — `.root`, `.rootMargin`,
   `.thresholds` are undefined; bundles may read `.thresholds` as an array.
4. **`ShadowRoot.getRootNode()` missing** — inherits from `DocumentFragment` which
   returns `ownerDocument`; for a ShadowRoot the non-composed result should be
   `this`.

## Non-goals

- Full Shadow DOM encapsulation (slotting, style scoping)
- Real IntersectionObserver geometry calculations
- Real ResizeObserver measurements
- Any changes outside `src/js_bootstrap.js`, `src/js.rs`, and `src/engine.rs`

## Context / Constraints

### What already exists (must NOT duplicate)
- `requestAnimationFrame` registered as native callback at `js.rs:1686` (→ `raf_cb`)
- `raf_cb` pushes to `RAF_TASKS` thread-local (js.rs:3192)
- `RAF_TASKS` drained inside `tick()` only when `timestamp: Some(_)` (js.rs:796)
- `send_tick()` is called by daemon's frame loop and `browser-cli tick N` command
- `window.cancelAnimationFrame` is already a no-op stub in `js_bootstrap.js:4398`

### What is missing
- `navigate()` never calls `tick_js()` — RAF callbacks from `init_js_for_page()`
  are never drained before `get_document_html()` reads the DOM
- `raf_cb` ignores `_rv` → returns `undefined` instead of an integer handle
- `IntersectionObserver` missing `root`, `rootMargin`, `thresholds` getters
- `ShadowRoot.getRootNode()` missing (falls through to Element base class impl
  which returns `ownerDocument || document`, not `this`)

## Approach (Checklist)

- [ ] **Step 0: Verify `init_js_for_page` RAF drain gap**
  Confirm `navigate()` in `engine.rs` never calls `tick_js()` between
  `init_js_for_page()` and `get_document_html()`.

- [ ] **Step 1: Add RAF ticks after `init_js_for_page()`** (`src/engine.rs`)
  After `init_js_for_page(&page)`, drain macro tasks + RAF in a short loop:
  ```rust
  // Drain macro tasks and RAF callbacks that React queued during init.
  // Cap at 5 ticks to avoid render-loop infinite drain.
  for _ in 0..5 {
      let ts = SystemTime::now()
          .duration_since(UNIX_EPOCH)
          .unwrap_or_default()
          .as_millis() as f64;
      if !self.tick_js(Some(ts), None) {
          break;
      }
  }
  ```

- [ ] **Step 2: Fix `raf_cb` return value** (`src/js.rs`)
  Add monotonically incrementing handle return, mirroring `ric_cb`:
  ```rust
  // In raf_cb, change signature:
  fn raf_cb(
      scope: &mut v8::PinScope,
      args: v8::FunctionCallbackArguments,
      mut rv: v8::ReturnValue<v8::Value>,  // was _rv
  ) {
      // ... existing push to RAF_TASKS ...
      // After the if block:
      let id = NEXT_RAF_ID.with(|c| { let id = *c.borrow(); *c.borrow_mut() += 1; id });
      rv.set_uint32(id);
  }
  ```
  Also add `static NEXT_RAF_ID: RefCell<u32> = RefCell::new(1);` in the
  thread_local! block.

- [ ] **Step 3: Fix `IntersectionObserver` property gaps** (`src/js_bootstrap.js`)
  Store options and add getters:
  ```js
  class IntersectionObserver {
      constructor(callback, options) {
          this._callback = callback;
          this._options = options || {};
      }
      get root() { return this._options.root || null; }
      get rootMargin() { return this._options.rootMargin || '0px'; }
      get thresholds() {
          var t = this._options.threshold;
          return t !== undefined ? [].concat(t) : [0];
      }
      observe(target) {}
      unobserve(target) {}
      disconnect() {}
      takeRecords() { return []; }
  }
  ```

- [ ] **Step 4: Fix `ShadowRoot.getRootNode()`** (`src/js_bootstrap.js`)
  Add override in the ShadowRoot class:
  ```js
  getRootNode(options) {
      if (options && options.composed) return document;
      return this;
  }
  ```

- [ ] **Step 5: Tests** (`src/js.rs`)
  - Test: `requestAnimationFrame` returns a number (not undefined)
  - Test: RAF callback fires after `tick(Some(0.0), None)`
  - Test: `IntersectionObserver` instance has `.thresholds` as array `[0]` by default
  - Test: `ShadowRoot` `getRootNode()` returns itself
  - Existing tests must remain green

- [ ] **Step 6: Smoke-verify Naver**
  - Start daemon, navigate to `https://www.naver.com/`
  - `browser-cli js "document.querySelectorAll('a').length"` → expect > 30
  - Screenshot non-white pixel count → expect > 10,000

## Validation

**Commands to run:**
```bash
cargo check --all-targets
cargo test --lib 2>&1 | tail -30
cargo build
# daemon should already be running
./target/debug/browser-cli navigate https://www.naver.com/
./target/debug/browser-cli js "document.querySelectorAll('a').length"
./target/debug/browser-cli screenshot /tmp/naver_after_256.png
python3 -c "
from PIL import Image
img = Image.open('/tmp/naver_after_256.png')
nw = [p for p in img.getdata() if not (p[0]>240 and p[1]>240 and p[2]>240)]
print(f'Non-white pixels: {len(nw)}')
"
```

**Expected output:**
- All `cargo test --lib` pass (including new tests)
- Link count > 30 (currently 8)
- Non-white pixel count > 10,000 (currently 22)

## Risks & Rollback

**Risks:**
- 5-tick RAF drain loop may not be enough if React needs more ticks. Tunable.
- Adding ticks increases `navigate()` latency by ~5 * tick_cost. Acceptable.
- If RAF drain causes infinite loop (render loop pattern), cap=5 prevents it.

**Rollback steps:**
- `git revert HEAD` — changes in `src/engine.rs` (tick loop) + `src/js.rs` (raf return)
  + `src/js_bootstrap.js` (IO getters + ShadowRoot.getRootNode). No DB, no flags.

## Revision notes (vs. original draft)

**Rejected from original plan:**
- Adding `window.requestAnimationFrame` JS shim in `js_bootstrap.js` → WRONG.
  Native `raf_cb` already registered (js.rs:1686). A JS shim at bootstrap end
  would shadow/overwrite the native function, breaking the RAF_TASKS drain path.
  Fix instead: add ticks in `navigate()`, fix return value in `raf_cb`.

## Open Questions

- None. Root cause confirmed via code inspection.
