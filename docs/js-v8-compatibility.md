# V8 Migration and JavaScript Compatibility Matrix

Status date: 2026-05-06

This document tracks the current runtime state after the Boa-to-V8 migration and
defines the boundary between completed V8 migration work and remaining ES module
or browser Web API work.

## Migration State

The active runtime is V8 through the `v8` crate. `Cargo.toml` depends on `v8`
and no longer depends on `boa_engine`. `JsRuntime` owns a V8 isolate and context
in `src/js.rs`, and the public execution API is still:

- `JsRuntime::new(...)`
- `JsRuntime::execute(...)`
- `JsRuntime::execute_with_result(...)`
- `JsRuntime::tick(...)`
- `JsRuntime::trigger_event(...)`

The old V8 phase issues `#233`, `#234`, `#235`, and `#236` are stale as
standalone GitHub issues. The priority roadmap already marks those phases done.
Remaining work now belongs to `#243` and its leaf issues `#245` through `#249`.

## Compatibility Matrix

| Capability | Status | Evidence / next issue |
|---|---|---|
| Inline classic scripts | Supported | `BrowserEngine::init_js_for_page` executes inline classic script sources. |
| Parser-inserted external classic scripts | Partial | External classic scripts are extracted in DOM order and fetched before execution. Ordering is basic; async/defer semantics need more work in `#247`. |
| Dynamic classic scripts | Partial | `__aura_maybe_run_script` handles appended classic scripts through `fetch()` and `eval()`. Lifecycle/error behavior needs `#247`. |
| V8 core execution | Supported | `JsRuntime::execute_with_result` compiles and runs classic scripts with V8. |
| DOM bindings | Partial | Core document/window/element APIs exist in `src/js_bootstrap.js` and native callbacks. Long-tail DOM parity remains outside this module issue. |
| Event/timer loop | Partial | `tick()` runs task queues, microtasks, animation frames, idle callbacks, and pending fetch callbacks. Full browser event-loop parity is incomplete. |
| Fetch API | Partial | `fetch()` supports basic request/response behavior and CORS checks. Full standards behavior remains incomplete. |
| XMLHttpRequest | Partial | XHR exists in bootstrap JS and uses fetch plumbing. Full browser parity remains incomplete. |
| URL / URLSearchParams / location | Partial | URL and location now resolve page and relative URLs; long-tail URL standard parity remains incomplete. |
| localStorage/sessionStorage | Partial | Origin storage exists, but storage event/session behavior is not complete. |
| Cookies / `document.cookie` | Missing or weak | Real JS-gated sites need a cookie jar and cookie semantics. Follow-up likely needed outside ES modules. |
| Inline module scripts | Partial | `#245` compiles inline module sources through V8 and caches by synthetic module URL. Browser lifecycle semantics remain `#247`. |
| External module scripts | Partial | `#245` resolves, CSP-checks, fetches, compiles, and caches external module sources. Graph linking/evaluation remains `#246`. |
| Static imports | Partial | `#245` reads V8 module requests during compile. Resolver/link/evaluate behavior remains `#246`. |
| Dynamic `import()` | Missing | Tracked by `#247`. |
| `import.meta.url` | Missing | Tracked by `#247`. |
| Top-level await | Missing | Tracked by `#246`. |
| Import maps | Missing | Not in first ES module milestone; should become a follow-up after `#247` unless needed earlier. |
| Service workers | Missing | Out of scope for `#243`; likely separate Web API issue. |
| WebSocket | Missing | Out of scope for `#243`; needed for some SPAs. |
| IndexedDB | Missing | Out of scope for `#243`; likely required for many modern apps. |

## Real-Site Blockers

### `https://yunseong.dev`

Current page content renders and basic form/navigation paths work. Remaining
risks are long-tail DOM, CSS, and JavaScript behavior on interactive pages such
as chat or mini-app routes. ES modules may matter for client-side bundles.

### `https://www.naver.com`

The static shell renders, but much of the portal experience depends on richer
JavaScript, event behavior, cookies/storage, and likely browser fingerprinting
or service integrations. ES module support is necessary but not sufficient.

### `https://chatgpt.com`

The page no longer fails at relative URL resolution for the Cloudflare challenge
script, but it still shows the JavaScript/cookies challenge message. Remaining
blockers likely include cookie semantics, browser environment APIs, challenge
runtime expectations, and possibly service-worker or fingerprinting surfaces.
These should be split into focused follow-up issues after `#249` verification.

## Stale Test Cleanup

The following tracked test files contained only block-commented Boa-era code and
no executable tests:

- `tests/repro_order.rs`
- `tests/test_boa.rs`
- `tests/test_event_loop.rs`
- `tests/test_event_loop_complex.rs`
- `tests/test_focus.rs`
- `tests/test_idle.rs`
- `tests/test_storage.rs`

They were removed because they referenced obsolete `boa_engine` APIs and
`JsRuntime.context`, which no longer exists. New tests should target the public
V8-backed runtime API or higher-level `BrowserEngine` behavior.

## Follow-Up Plan

1. `#245`: add the ES module loader foundation.
2. `#246`: link and evaluate module graphs.
3. `#247`: implement browser module semantics.
4. `#248`: integrate modules with DOM/style/tick behavior.
5. `#249`: run fixture and real-site verification, then split non-module Web API
   blockers into dedicated issues.
