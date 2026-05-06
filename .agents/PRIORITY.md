# Issue Priority Roadmap

Completed history archived in [PRIORITY-ARCHIVE.md](./PRIORITY-ARCHIVE.md) (Priorities 0–13, ~120 issues resolved).

Priority is ordered by: **foundational value → dependency order → visual impact**.

---

## Priority 14 - V8 JS Engine Migration

Umbrella tracker: #232 `[JS] Migrate JS engine from Boa to V8 (rusty_v8)`

Replace `boa_engine` (pure-Rust interpreter, limited ES6) with `rusty_v8` (Chrome's V8 via prebuilt bindings). Full ES2024 + JIT. Unblocks modern SPA pages (chatgpt.com, etc.).

Must be done in order — each phase builds on the previous.

| # | Issue | Why this order |
|---|---|---|
| #233 ✓ | [V8] Phase 1: Set up rusty_v8 + isolate + script execution | Foundation. Cargo.toml + core script eval. |
| #234 ✓ | [V8] Phase 2: Port DOM bindings (document, window, element) | Depends on #233. Need DOM before events. |
| #235 ✓ | [V8] Phase 3: Port event system + timers | Depends on #234. Events need DOM targets. |
| #236 ✓ | [V8] Phase 4: Port fetch, XHR, storage, CSSOM, form APIs | Depends on #235. Network/storage need stable event loop. |
| #237 ✓ | [V8] Phase 5: Integration + real-world testing | Depends on #236. Final verification against real sites. |

## Dependency graph

```
#233 -> #234 -> #235 -> #236 -> #237
```

## Domain closure

| Domain issue | Closes when |
|---|---|---|
| Complete V8 + ES Modules ✓ | #244 #245 #246 #247 #248 #249 done |

---

## Priority 16 - Real-Site Playwright Visual Parity

Umbrella tracker: #252 `[Real-site] Playwright visual parity for www.naver.com and yunseong.dev`

Make `https://www.naver.com` and `https://yunseong.dev` visually close to Playwright/Chromium baselines. This depends on ES module work plus broader DOM/Web API compatibility.

Must be done in order — API blockers first, then site fixtures.

| # | Issue | Why this order |
|---|---|---|
| #253 ✓ | [DOM] Add CharacterData/Text/Comment constructor and prototype parity | Real bundles and polyfills fail early when DOM constructor globals are missing. |
| #254 ✓ | [DOM] Add form APIs and event handler properties used by real sites | Fixes `onsubmit`/handler-property gaps that block form and framework initialization. |
| #255 ✓ | [DOM] Implement iframe contentWindow/contentDocument foundation | Fixes Cloudflare-style hidden iframe scripts on yunseong.dev. |
| #256 | [DOM] Add observer and ShadowRoot compatibility for modern bundles (in progress by opencode:deepseek-v4-pro) | Modern bundles probe ShadowRoot, getRootNode, IntersectionObserver, and ResizeObserver. |
| #257 | [Real-site] Naver homepage Playwright parity fixture and first-pass rendering | Depends on #253-#256. Naver shell is mostly JS-populated. |
| #258 | [Real-site] yunseong.dev dynamic JS parity against Playwright baseline | Depends on #149, #253-#256, and ES module graph/browser semantics. |

## Dependency graph

```
#253 -> #254 -> #255 -> #256 -> #257
                         └──────-> #258
#246 -> #247 -> #248 ─────────────> #258
```

## Domain closure

| Domain issue | Closes when |
|---|---|
| Real-site Playwright parity | #252 #253 #254 #255 #256 #257 #258 done |
