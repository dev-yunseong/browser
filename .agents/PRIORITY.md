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
|---|---|
| V8 Migration | #232 #233 #234 #235 #236 #237 done |

---

## Priority 15 - Complete V8 Migration + ES Module Support

Umbrella tracker: #243 `[JS] Complete V8 migration and ES module support` (in progress by codex:gpt-5)

The runtime now embeds V8 for classic scripts, but ES modules and full modern JavaScript loading semantics remain incomplete. This priority is the new source of truth for finishing the migration after #232.

Compatibility matrix: [docs/js-v8-compatibility.md](../docs/js-v8-compatibility.md)

Must be done in order — each phase builds on the previous.

| # | Issue | Why this order |
|---|---|---|
| #244 | [JS] V8 migration audit and compatibility matrix (in progress by codex:gpt-5) | Foundation. Clarifies stale phase issues and current support gaps. |
| #245 | [JS] ES module loader foundation | Depends on #244. Needs resolver, fetch, CSP, cache, and V8 module compile path. |
| #246 | [JS] ES module graph linking and evaluation | Depends on #245. Needs module graph/link/evaluate before browser semantics. |
| #247 | [JS] Browser ES module semantics | Depends on #246. Adds import.meta, dynamic import, nomodule, and script lifecycle behavior. |
| #248 | [JS] Integrate ES modules with DOM mutation, style, and tick | Depends on #247. Modules must affect rendering and async work through the event loop. |
| #249 | [JS] ES module real-world verification | Depends on #248. Final fixture and live-site verification. |

## Dependency graph

```
#244 -> #245 -> #246 -> #247 -> #248 -> #249 -> #243
```

## Domain closure

| Domain issue | Closes when |
|---|---|
| Complete V8 + ES Modules | #244 #245 #246 #247 #248 #249 done |
