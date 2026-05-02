# Issue Priority Roadmap

Priority is ordered by: **foundational value -> dependency order -> visual impact**.

---

## Priority 0 - Daemon / CLI Architecture (Highest)

Must be done in order. Foundational infrastructure - unlocks headless testing, CLI access, and the browser-tester agent.

| # | Issue | Why first |
|---|---|---|
| #68 | ~~Extract Headless BrowserEngine~~ ✓ | Decouples engine from GUI. All other daemon/CLI work depends on this. |
| #70 | ~~Browser Daemon (engine + GUI + HTTP server)~~ ✓ | Single process: engine + eframe GUI + axum HTTP. CLI and GUI share state. |
| #72 | ~~Refactor BrowserApp as daemon GUI module~~ ✓ | BrowserApp uses `Arc<Mutex<BrowserEngine>>` directly. No engine logic in main.rs. |
| #69 | ~~CLI HTTP client (Markdown + REPL)~~ ✓ | Thin HTTP client to daemon. Markdown renderer + interactive REPL. |
| #71 | ~~Browser Tester Agent~~ ✓ | Project-local skill. Drives CLI to test all browser features. |

---

## Priority 1 - Layout Correctness

These unblock domain issue #4 (Complete Layout Contexts).

| # | Issue | Why first |
|---|---|---|
| #13 | ~~Intrinsic & Extrinsic Sizing~~ ✓ | `min-content`/`max-content` needed for correct table, image, and shrink-wrap sizing. No dependencies. |

---

## Priority 2 - Compositor Pipeline

Must be done in order - each issue depends on the previous.
Together they close domain issue #5 (High-Fidelity Rendering).

| # | Issue | Why this order |
|---|---|---|
| #30 | ~~Layer Tree Builder~~ ✓ | Defines `Layer`/`LayerTree` structs. Everything else builds on this. |
| #32 | ~~Compositor Transform & Matrix Engine~~ ✓ | Matrix math needed by #33 for applying transforms/opacity. |
| #31 | ~~Tile & Texture Management~~ ✓ | Texture pool needed by #33 for drawing layer content. |
| #33 | ~~Layer Blending & Composition Loop~~ ✓ | Final step - composites all layers to screen. Requires #30, #31, #32. |

---

## Priority 3 - Event Loop

Must be done in order. Closes domain issue #6 (Runtime & Interactivity).

| # | Issue | Why this order |
|---|---|---|
| #22 | ~~Event Loop: Macro/Micro Task Integration~~ ✓ | Foundation. #38 and #40 both depend on this. |
| #38 | ~~Render-step Orchestrator~~ ✓ | `requestAnimationFrame` - depends on #22's task queue. |
| #40 | ~~Idle Callback Scheduler~~ ✓ | `requestIdleCallback` - depends on #22's idle slot detection. |
| #25 | ~~Focus Management & Sequential Navigation~~ ✓ | Tab/focus state. Depends on event loop being stable. |

---

## Priority 4 - Security Model

All three are independent of each other. Closes domain issue #7.

| # | Issue | Why this order |
|---|---|---|
| #27 | ~~CORS Enforcement Layer~~ ✓ | Most commonly hit by real pages. |
| #28 | ~~Origin-Based Storage & SOP~~ ✓ | Builds on CORS concepts. |
| #29 | ~~Content Security Policy Engine~~ ✓ | Additive on top of #27/#28. |

---

## Priority 5 - Parser

| # | Issue | Why last |
|---|---|---|
| #19 | ~~WHATWG-compliant HTML5 Tokenizer~~ ✓ | `html5ever` already handles this correctly. Custom implementation is spec completeness, not a visible improvement. |

---

## Priority 6 - HTML & CSS Engine Perfection (Performance & Stability)

Addresses severe crashes (OOM/Stack overflow) and massive rendering latency (>1.5s) on real-world pages.

| # | Issue | Why this order |
|---|---|---|
| #60 | ~~Parallel Async CSS Fetching & Processing~~ ✓ | Blocking the main thread for 1.3s during CSS parsing makes the browser unusable. Fix network first. |
| #61 | ~~Style Tree Memory Optimization (OOM)~~ ✓ | Creating HashMaps for every DOM node causes massive OOM on large sites. |
| #62 | ~~CSS Parser & Selector Matching Opt~~ ✓ | O(N*M) selector matching is painfully slow. We need Right-to-Left matching and caches. |
| #63 | ~~Layout Engine Recursion & Scalability~~ ✓ | Deeply nested DOMs crash the layout engine with stack overflows. Convert to iterative trees. |

---

## Priority 7 - Google.com Layout Fidelity

Sub-issues of #103. Fix in order — each builds on the previous layer of visible correctness.

| # | Issue | Why this order |
|---|---|---|
| #103 | [Layout] google.com search page layout fidelity | Umbrella tracker for the remaining Google layout work. |
| #110 | ~~[Layout] google.com — form control intrinsic sizing~~ ✓ | Most foundational. Search input collapsed = nothing else matters. |
| #111 | ~~[Layout] google.com — absolute/fixed header positioning~~ ✓ | Affects nav placement after baseline sizing is correct. |
| #112 | ~~[Layout] google.com — spacing and line-box alignment in dense header~~ ✓ | Refinement after positioning is correct. |
| #113 | ~~[Runtime] google.com — pre-layout DOM/runtime parity gaps~~ ✓ | Runtime parity layer completed before the remaining visual follow-ups. |
| #119 | [Runtime] issue #113 follow-up — stateful history, text node mutation, URL/base correctness (in progress by claude-sonnet-4-6) | Follow-up to #113 — fixes incoherent history/location state, silent DOM mutation failure, and URL resolution bugs. |
| #124 | ~~[Layout] google.com — hero/logo and search cluster composition~~ ✓ | Fixed the missing top-center Google logo/hero composition in headless rendering. |
| #127 | ~~[Layout] google.com — search row control sizing and inline alignment~~ ✓ | Fixed the `<br>`-driven button row break so the search controls stay grouped below the input. |
| #126 | ~~[Layout] google.com — stray right-edge overflow artifact~~ ✓ | Isolated visual overflow cleanup after the primary structure is corrected. |
| #125 | ~~[Layout] google.com — footer anchoring and link grouping~~ ✓ | Lowest user-impact remaining defect once the hero/search region is stable. |
| #134 | [Layout] google.com — header utility cluster anchoring and right-edge clipping (in progress by codex:gpt-5.4) | Most visible remaining defect in the current screenshot; clipped header controls break the page structure at first glance. |
| #133 | [Layout] google.com — advanced search link escapes the centered hero controls (in progress by codex:gpt-5.4) | Next most visible hero-structure bug after the header cluster is anchored. |
| #132 | [Render] google.com — search action button labels not painted | Control text rendering after layout positions are stable. |
| #131 | [Layout] google.com — footer legal copy spacing and line grouping | Lowest-impact remaining cleanup after header/hero/button fidelity is restored. |

## Priority 8 - GUI Render Stability

Stabilizes same-page rendering so hover/focus/image-triggered updates do not relayout the page with inconsistent viewport widths.

| # | Issue | Why this order |
|---|---|---|
| #137 | [Runtime] stabilize viewport width across re-renders to prevent layout jitter | Fixes visible component size jitter and removes unnecessary full-page relayouts caused by mixed re-render widths. |

---

## Priority 9 - CSS Engine Completeness

| # | Issue | Why this order |
|---|---|---|
| #143 | ~~[CSS] CSS custom properties (CSS variables) support~~ ✓ | Already implemented; PR added tests. |
| #142 | ~~[Layout] Basic flexbox row layout~~ ✓ | Gmail/이미지 now inline. Full flex support with align/justify/gap/order. |
| #145 | ~~[CSS] border-radius on form controls~~ ✓ | Fixed CSS parser — shorthand was parsed as keyword. |
| #144 | ~~[CSS] CSS transform property support~~ ✓ | Already in #153; PR closed issue + fixed flaky render test. |
| #150 | ~~[CSS] @media query parsing and evaluation~~ ✓ | Responsive CSS activation. yunseong.dev dark navbar/hero needs this. |
| #156 | ~~[CSS] @media (prefers-color-scheme: dark) support~~ ✓ | Headless renderer defaults to dark; dark-scheme rules now activate. |
| #151 | [Render] box-shadow support | Card depth/elevation. yunseong.dev content card flat without it. |
| #152 | [Layout] list-style-type and list indentation | Bullet markers missing on yunseong.dev project lists. |
| #158 | ~~[CSS] ::before and ::after pseudo-element support~~ ✓ | Clearfix and decorative content. Closed by PR #162. |

---

## Priority 10 - DevTools

| # | Issue | Why this order |
|---|---|---|
| #107 | ~~[DevTools] Developer console panel — display JS console output~~ ✓ | Foundation for DevTools. #108 depends on this. |
| #108 | ~~[DevTools] Console REPL — execute JS from developer console~~ ✓ | Depends on #107 console panel. |

---

## Dependency graph

```
#68 -> #70 -> #72
#68 -> #70 -> #69 -> #71
#13
#30 -> #32 -> #31 -> #33
#22 -> #38
#22 -> #40
#22 -> #25
#27
#28
#29
#19
#60 -> #61 -> #62 -> #63
```

## Domain closure

| Domain issue | Closes when |
|---|---|
| Daemon/CLI Infrastructure | #68 #69 #70 #71 #72 done |
| #4 Complete Layout Contexts | #13 done |
| #5 High-Fidelity Rendering | #30 #31 #32 #33 done |
| #6 Runtime & Interactivity | #22 #25 #38 #40 done |
| #7 Resource Loading & Security | #27 #28 #29 done |
| #2 Standard HTML5 & CSS Parsing | #19 done |
| Perfecting Engine (Performance) | #60 #61 #62 #63 done |
| #1 Vision | all domains done |
