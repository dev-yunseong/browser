# Plan - Fix CORS Enforcement on Dynamic Classic Scripts

Naver's React app fails to mount because a dynamically inserted classic script tag (`ndp-core.js` from `ssl.pstatic.net`) is loaded via `fetch` in the JS bootstrap script. The browser's native `__aura_fetch` implementation blocks it because it is cross-origin and does not pass CORS checks.
However, in standard browsers, classic script tags (`<script src="...">`) do not enforce CORS. We should allow dynamic classic scripts to bypass CORS checks.

## Proposed Changes

### 1. `src/js.rs`
Modify `fetch_cb` to accept a 7th argument, `bypass_cors` (boolean):
- Read the 7th argument `bypass_cors` (default to `false` if not provided).
- Skip the CORS allowed check (line 3110-3123) if `bypass_cors` is `true`.

### 2. `src/js_bootstrap.js`
Modify `__aura_maybe_run_script` to:
- Call `__aura_fetch` directly with the `bypassCors` flag instead of using page-level `fetch(src)`.
- Determine `bypassCors` as `!isModule && !script.hasAttribute('crossorigin')` (modules and classic scripts with `crossorigin` attribute enforce CORS; other classic scripts do not).
- Improve error logging to include the script URL on load/execution failures.

## Verification Plan
1. Compile and run cargo check / tests.
2. Restart the daemon on Naver.
3. Navigating to Naver, then tick JS to allow all dynamic scripts to execute.
4. Capture a new screenshot to verify that the home page content is loaded and rendered instead of a blank screen.
