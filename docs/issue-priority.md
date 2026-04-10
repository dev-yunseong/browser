# Issue Priority Roadmap

Priority is ordered by: **foundational value → dependency order → visual impact**.

---

## Priority 1 — Layout Correctness

These unblock domain issue #4 (Complete Layout Contexts).

| # | Issue | Why first |
|---|---|---|
| #13 | Intrinsic & Extrinsic Sizing | `min-content`/`max-content` needed for correct table, image, and shrink-wrap sizing. No dependencies. |

---

## Priority 2 — Compositor Pipeline

Must be done in order — each issue depends on the previous.
Together they close domain issue #5 (High-Fidelity Rendering).

| # | Issue | Why this order |
|---|---|---|
| #30 | Layer Tree Builder | Defines `Layer`/`LayerTree` structs. Everything else builds on this. |
| #32 | Compositor Transform & Matrix Engine | Matrix math needed by #33 for applying transforms/opacity. |
| #31 | Tile & Texture Management | Texture pool needed by #33 for drawing layer content. |
| #33 | Layer Blending & Composition Loop | Final step — composites all layers to screen. Requires #30, #31, #32. |

---

## Priority 3 — Event Loop

Must be done in order. Closes domain issue #6 (Runtime & Interactivity).

| # | Issue | Why this order |
|---|---|---|
| #22 | Event Loop: Macro/Micro Task Integration | Foundation. #38 and #40 both depend on this. |
| #38 | Render-step Orchestrator | `requestAnimationFrame` — depends on #22's task queue. |
| #40 | Idle Callback Scheduler | `requestIdleCallback` — depends on #22's idle slot detection. |
| #25 | Focus Management & Sequential Navigation | Tab/focus state. Depends on event loop being stable. |

---

## Priority 4 — Security Model

All three are independent of each other. Closes domain issue #7.

| # | Issue | Why this order |
|---|---|---|
| #27 | CORS Enforcement Layer | Most commonly hit by real pages. |
| #28 | Origin-Based Storage & SOP | Builds on CORS concepts. |
| #29 | Content Security Policy Engine | Additive on top of #27/#28. |

---

## Priority 5 — Parser

| # | Issue | Why last |
|---|---|---|
| #19 | WHATWG-compliant HTML5 Tokenizer | `html5ever` already handles this correctly. Custom implementation is spec completeness, not a visible improvement. |

---

## Dependency graph

```
#13
#30 → #32 → #31 → #33
#22 → #38
#22 → #40
#22 → #25
#27
#28
#29
#19
```

## Domain closure

| Domain issue | Closes when |
|---|---|
| #4 Complete Layout Contexts | #13 done |
| #5 High-Fidelity Rendering | #30 #31 #32 #33 done |
| #6 Runtime & Interactivity | #22 #25 #38 #40 done |
| #7 Resource Loading & Security | #27 #28 #29 done |
| #2 Standard HTML5 & CSS Parsing | #19 done |
| #1 Vision | all domains done |
