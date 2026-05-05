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
| #233 | [V8] Phase 1: Set up rusty_v8 + isolate + script execution (in progress by sisyphus:deepseek-v4-pro) | Foundation. Cargo.toml + core script eval. |
| #234 | [V8] Phase 2: Port DOM bindings (document, window, element) | Depends on #233. Need DOM before events. |
| #235 | [V8] Phase 3: Port event system + timers | Depends on #234. Events need DOM targets. |
| #236 | [V8] Phase 4: Port fetch, XHR, storage, CSSOM, form APIs | Depends on #235. Network/storage need stable event loop. |
| #237 | [V8] Phase 5: Integration + real-world testing | Depends on #236. Final verification against real sites. |

## Dependency graph

```
#233 -> #234 -> #235 -> #236 -> #237
```

## Domain closure

| Domain issue | Closes when |
|---|---|
| V8 Migration | #232 #233 #234 #235 #236 #237 done |
