# Issue Priority Roadmap

Priority is ordered by: **foundational value → dependency order → visual impact**.

---

## Priority 0 — Daemon / CLI Architecture (Highest)

Must be done in order. Foundational infrastructure — unlocks headless testing, CLI access, and the browser-tester agent.

| # | Issue | Why first |
|---|---|---|
| #68 | ~~Extract Headless BrowserEngine~~ ✓ | Decouples engine from GUI. All other daemon/CLI work depends on this. |
| #70 | ~~Browser Daemon (engine + GUI + HTTP server)~~ ✓ | Single process: engine + eframe GUI + axum HTTP. CLI and GUI share state. |
| #72 | ~~Refactor BrowserApp as daemon GUI module~~ ✓ | BrowserApp uses `Arc<Mutex<BrowserEngine>>` directly. No engine logic in main.rs. |
| #69 | ~~CLI HTTP client (Markdown + REPL)~~ ✓ | Thin HTTP client to daemon. Markdown renderer + interactive REPL. |
| #71 | ~~Browser Tester Agent~~ ✓ | Project-local skill. Drives CLI to test all browser features. |

---

## Priority 1 — Layout Correctness

| # | Issue | Why first |
|---|---|---|
| #13 | ~~Intrinsic & Extrinsic Sizing~~ ✓ | `min-content`/`max-content` needed for correct table, image, and shrink-wrap sizing. |

---

## Priority 2 — Visual Fidelity (Box Model & Rendering)

Make the browser look like a real browser. Independent issues — do in any order within this group.
Together they close domain issue #5 (High-Fidelity Rendering) and much of #4 (Layout Contexts).

| # | Issue | Why this order |
|---|---|---|
| #82 | ~~Text Decoration & Font Variants~~ ✓ | Highest impact — bold/italic/underline visible on every page. No dependencies. |
| #83 | ~~Image: object-fit, aspect ratio, fallback~~ ✓ | Fixes distorted images. No dependencies. |
| #86 | ~~position: absolute / fixed / relative / sticky~~ ✓ | Modals, dropdowns, navbars all broken without this. |
| #87 | ~~overflow: hidden — clip content to box~~ ✓ | Content bleeds outside cards/containers. |
| #88 | ~~z-index stacking context~~ ✓ | Overlapping elements in wrong order. |
| #90 | ~~Margin collapsing~~ ✓ | Doubled spacing everywhere on body text. |
| #81 | CSS Gradient Background | Buttons, hero sections. No dependencies. |
| #89 | box-shadow Gaussian blur | Soft shadow edges. No dependencies. |
| #85 | Flexbox Completion — wrap, align, justify, gap | Navbars/card grids look wrong. |
| #84 | DOM API — querySelector, classList, addEventListener | JS-driven UI. |

---

## Priority 3 — Compositor Pipeline

Must be done in order — each issue depends on the previous.
Together they close domain issue #5 (High-Fidelity Rendering).

| # | Issue | Why this order |
|---|---|---|
| #30 | ~~Layer Tree Builder~~ ✓ | Defines `Layer`/`LayerTree` structs. Everything else builds on this. |
| #32 | ~~Compositor Transform & Matrix Engine~~ ✓ | Matrix math needed by #33 for applying transforms/opacity. |
| #31 | ~~Tile & Texture Management~~ ✓ | Texture pool needed by #33 for drawing layer content. |
| #33 | ~~Layer Blending & Composition Loop~~ ✓ | Final step — composites all layers to screen. Requires #30, #31, #32. |

---

## Priority 4 — Event Loop

Must be done in order. Closes domain issue #6 (Runtime & Interactivity).

| # | Issue | Why this order |
|---|---|---|
| #22 | ~~Event Loop: Macro/Micro Task Integration~~ ✓ | Foundation. #38 and #40 both depend on this. |
| #38 | ~~Render-step Orchestrator~~ ✓ | `requestAnimationFrame` — depends on #22's task queue. |
| #40 | ~~Idle Callback Scheduler~~ ✓ | `requestIdleCallback` — depends on #22's idle slot detection. |
| #25 | ~~Focus Management & Sequential Navigation~~ ✓ | Tab/focus state. Depends on event loop being stable. |

---

## Priority 5 — Security Model

All three are independent of each other. Closes domain issue #7.

| # | Issue | Why this order |
|---|---|---|
| #27 | ~~CORS Enforcement Layer~~ ✓ | Most commonly hit by real pages. |
| #28 | ~~Origin-Based Storage & SOP~~ ✓ | Builds on CORS concepts. |
| #29 | ~~Content Security Policy Engine~~ ✓ | Additive on top of #27/#28. |

---

## Priority 6 — Parser

| # | Issue | Why last |
|---|---|---|
| #19 | ~~WHATWG-compliant HTML5 Tokenizer~~ ✓ | `html5ever` already handles this correctly. Custom implementation is spec completeness, not a visible improvement. |

---

## Priority 7 — HTML & CSS Engine Perfection (Performance & Stability)

Addresses severe crashes (OOM/Stack overflow) and massive rendering latency (>1.5s) on real-world pages.

| # | Issue | Why this order |
|---|---|---|
| #60 | ~~Parallel Async CSS Fetching & Processing~~ ✓ | Blocking the main thread for 1.3s during CSS parsing makes the browser unusable. Fix network first. |
| #61 | ~~Style Tree Memory Optimization (OOM)~~ ✓ | Creating HashMaps for every DOM node causes massive OOM on large sites. |
| #62 | ~~CSS Parser & Selector Matching Opt~~ ✓ | O(N*M) selector matching is painfully slow. We need Right-to-Left matching and caches. |
| #63 | Layout Engine Recursion & Scalability | Deeply nested DOMs crash the layout engine with stack overflows. Convert to iterative trees. |

---

## Dependency graph

```
#68 → #70 → #72
#68 → #70 → #69 → #71
#13
#82, #83, #86, #87, #88, #89, #90 (independent)
#81, #84, #85 (independent)
#30 → #32 → #31 → #33
#22 → #38
#22 → #40
#22 → #25
#27
#28
#29
#19
#60 → #61 → #62 → #63
```

## Domain closure

| Domain issue | Closes when |
|---|---|
| Daemon/CLI Infrastructure | #68 #69 #70 #71 #72 done ✓ |
| #4 Complete Layout Contexts | #13 #86 #87 #90 done ✓ |
| #5 High-Fidelity Rendering | #81 #82 #83 #85 #88 #89 #30 #31 #32 #33 done |
| #6 Runtime & Interactivity | #22 #25 #38 #40 done ✓ |
| #7 Resource Loading & Security | #27 #28 #29 done ✓ |
| #2 Standard HTML5 & CSS Parsing | #19 done ✓ |
| Perfecting Engine (Performance) | #60 #61 #62 #63 done |
| #1 Vision | all domains done |
