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
| #103 | ~~[Layout] google.com search page layout fidelity~~ ✓ | Umbrella tracker for the remaining Google layout work. |
| #110 | ~~[Layout] google.com — form control intrinsic sizing~~ ✓ | Most foundational. Search input collapsed = nothing else matters. |
| #111 | ~~[Layout] google.com — absolute/fixed header positioning~~ ✓ | Affects nav placement after baseline sizing is correct. |
| #112 | ~~[Layout] google.com — spacing and line-box alignment in dense header~~ ✓ | Refinement after positioning is correct. |
| #113 | ~~[Runtime] google.com — pre-layout DOM/runtime parity gaps~~ ✓ | Runtime parity layer completed before the remaining visual follow-ups. |
| #119 | ~~[Runtime] issue #113 follow-up — stateful history, text node mutation, URL/base correctness~~ ✓ | Follow-up to #113 — fixes incoherent history/location state, silent DOM mutation failure, and URL resolution bugs. |
| #124 | ~~[Layout] google.com — hero/logo and search cluster composition~~ ✓ | Fixed the missing top-center Google logo/hero composition in headless rendering. |
| #127 | ~~[Layout] google.com — search row control sizing and inline alignment~~ ✓ | Fixed the `<br>`-driven button row break so the search controls stay grouped below the input. |
| #126 | ~~[Layout] google.com — stray right-edge overflow artifact~~ ✓ | Isolated visual overflow cleanup after the primary structure is corrected. |
| #125 | ~~[Layout] google.com — footer anchoring and link grouping~~ ✓ | Lowest user-impact remaining defect once the hero/search region is stable. |
| #134 | ~~[Layout] google.com — header utility cluster anchoring and right-edge clipping (in progress by codex:gpt-5.4)~~ ✓ | Most visible remaining defect in the current screenshot; clipped header controls break the page structure at first glance. |
| #133 | ~~[Layout] google.com — advanced search link escapes the centered hero controls (in progress by codex:gpt-5.4)~~ ✓ | Next most visible hero-structure bug after the header cluster is anchored. |
| #132 | ~~[Render] google.com — search action button labels not painted~~ ✓ | Control text rendering after layout positions are stable. |
| #131 | ~~[Layout] google.com — footer legal copy spacing and line grouping~~ ✓ | Lowest-impact remaining cleanup after header/hero/button fidelity is restored. |

## Priority 8 - GUI Render Stability

Stabilizes same-page rendering so hover/focus/image-triggered updates do not relayout the page with inconsistent viewport widths.

| # | Issue | Why this order |
|---|---|---|
| #137 | ~~[Runtime] stabilize viewport width across re-renders to prevent layout jitter~~ ✓ | Fixes visible component size jitter and removes unnecessary full-page relayouts caused by mixed re-render widths. |

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
| #157 | ~~[Layout] CSS Grid layout support~~ ✓ | display:grid with fr/repeat/gap. PR #164. |
| #151 | ~~[Render] box-shadow support~~ ✓ | Card depth/elevation. yunseong.dev content card flat without it. |
| #152 | ~~[Layout] list-style-type and list indentation~~ ✓ | Bullet markers missing on yunseong.dev project lists. |
| #158 | ~~[CSS] ::before and ::after pseudo-element support~~ ✓ | Clearfix and decorative content. Closed by PR #162. |

---

## Priority 10 - DevTools

| # | Issue | Why this order |
|---|---|---|
| #107 | ~~[DevTools] Developer console panel — display JS console output~~ ✓ | Foundation for DevTools. #108 depends on this. |
| #108 | ~~[DevTools] Console REPL — execute JS from developer console~~ ✓ | Depends on #107 console panel. |

---

## Priority 11 - DOM Core Completeness

Umbrella tracker: #165 `[DOM] DOM implementation completeness tracker` ✓

Must be done in order. Core node-model correctness comes before selector, event, and geometry parity.

| # | Issue | Why this order |
|---|---|---|
| #185 | ~~[DOM] real DocumentFragment semantics and insertion behavior (in progress by codex:gpt-5)~~ ✓ | First concrete slice of #166. Fragment semantics are the biggest structural correctness gap. |
| #186 | ~~[DOM] JS-visible Comment and DocumentType node support (in progress by codex:gpt-5)~~ ✓ | Expands node-kind coverage after fragment support is real. |
| #187 | ~~[DOM] sibling navigation APIs and node relationship parity (in progress by codex:gpt-5)~~ ✓ | Depends on the core node model being coherent for all basic node kinds. |
| #188 | ~~[DOM] document surface semantics (`documentElement`, `head`, `body`) (in progress by codex:gpt-5)~~ ✓ | Final document-surface cleanup within #166 after the underlying node relationships are stable. |
| #166 | ~~[DOM] core node model, document surface, and fragment correctness~~ ✓ | Umbrella parent for #185–#188. Foundation for all remaining DOM work. |
| #167 | ~~[DOM] unify selector behavior across CSS matching and JS query APIs (in progress by codex:gpt-5)~~ ✓ | Query/style parity depends on the core node/document model being stable. |
| #194 | ~~[DOM] cloneNode(deep) mutation correctness (in progress by codex:gpt-5)~~ ✓ | First concrete slice of #168. Cloning semantics should be stable before broader subtree mutation cleanup. |
| #195 | ~~[DOM] fragment insertion mutation invariants (in progress by codex:gpt-5)~~ ✓ | Re-check fragment mutation paths after clone semantics are solid. |
| #196 | ~~[DOM] innerHTML subtree replacement invariants (in progress by codex:gpt-5)~~ ✓ | Replacement correctness depends on fragment and clone invariants staying coherent. |
| #197 | ~~[DOM] detached subtree and node adoption semantics (in progress by codex:gpt-5)~~ ✓ | Final mutation-state cleanup after clone/fragment/replacement behavior is predictable. |
| #168 | ~~[DOM] DOM mutation correctness for clone, fragment, and subtree operations (in progress by codex:gpt-5)~~ ✓ | Needs #166 fragment/node semantics in place first. |
| #169 | ~~[DOM] serialization fidelity for innerHTML, outerHTML, and textContent (in progress by codex:gpt-5)~~ ✓ | Safer after core mutation semantics are correct. |
| #170 | ~~[DOM] event dispatch phases and richer DOM event classes (in progress by codex:gpt-5)~~ ✓ | Depends on stable tree relationships and node identity guarantees from #166. |
| #171 | ~~[DOM] layout-backed geometry APIs for JS (in progress by codex:gpt-5)~~ ✓ | Depends on stable DOM/layout mapping after the core model and mutation paths settle. |
| #205 | ~~[DOM] HTMLCollection and live element collection behavior~~ ✓ | First concrete slice of #172. Live element collections should be stable before traversal APIs build on collection behavior. |
| #206 | ~~[DOM] TreeWalker traversal over live DOM tree~~ ✓ | TreeWalker exposes explicit cursor traversal over the current DOM tree. |
| #208 | ~~[DOM] NodeIterator traversal over live DOM tree~~ ✓ | NodeIterator is adjacent to TreeWalker but has different cursor semantics. |
| #207 | ~~[DOM] Range API basics~~ ✓ | Range is the broadest #172 child and should land after traversal support. |
| #172 | ~~[DOM] collections and traversal APIs (split into #205-#208)~~ ✓ | Best done after the core node model is solid. |
| #173 | ~~[DOM] MutationObserver support~~ ✓ | Depends on mutation semantics and event-loop delivery behavior already being stable. |
| #175 | ~~[DOM] form DOM state parity~~ ✓ | Depends on mutation/event/core DOM work for correct control behavior. |
| #174 | ~~[DOM] shadow DOM support~~ ✓ | Most invasive DOM feature; keep last in DOM track. |

---

## Priority 12 - Browser JS / Web API Completeness

Umbrella tracker: #176 `[JS] browser JavaScript Web API completeness tracker` ✓

| # | Issue | Why this order |
|---|---|---|
| #180 | ~~[JS] URL, URLSearchParams, and location API parity~~ ✓ | URL/base resolution is foundational and already intersects prior runtime bugs. |
| #181 | ~~[JS] History API and navigation event parity~~ ✓ | Builds on URL/location correctness. |
| #179 | ~~[JS] cookie jar and document.cookie semantics~~ ✓ | Core page/session state needed before broader network parity. |
| #177 | ~~[JS] Web Fetch API completion~~ ✓ | Main network API. Better after URL/cookie basics are coherent. |
| #178 | ~~[JS] XMLHttpRequest support~~ ✓ | Can reuse pieces of the fetch/network stack. |
| #182 | ~~[JS] CSSOM JS surface completion~~ ✓ | Best after DOM core and style read paths improve. |
| #183 | ~~[JS] window, navigator, and screen environment parity~~ ✓ | Lower-risk environment surface once stateful APIs exist. |
| #184 | ~~[JS] dynamic script loading and module support~~ ✓ | Most integration-heavy JS feature; keep last in JS track. |

---

## Priority 13 - Google Search (Form Submission + Navigation)

Umbrella tracker: #224 `[Feature] Google Search — form submission and search navigation`

Must be done in order — form metadata extraction unblocks submission, which unblocks GUI integration.
URL bar search fallback is independent.

| # | Issue | Why this order |
|---|---|---|
| #225 | ~~[Form] Extract form metadata (action, method, field names) in PageResult~~ ✓ | Foundation. Form submission needs action/method/field names from DOM. |
| #226 | [Form] EngineCmd::Submit — form URL construction and submission (in progress by opencode:deepseek-v4-pro) | Depends on #225 metadata. Builds the navigation URL from form state. |
| #227 | [GUI] Enter-key form submission in GUI and daemon | Depends on #226. Wires Enter key → submit → navigate in GUI. |
| #228 | [Navigation] URL bar search fallback (non-URL → Google search) | Independent. Simple URL detection heuristic. |

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
#166 -> #185 -> #186 -> #187 -> #188
#166 -> #167 -> #194 -> #195 -> #196 -> #197 -> #168 -> #169
#166 -> #170
#166 -> #171
#166 -> #172
#168 -> #173
#166 -> #175
#180 -> #181
#180 -> #179 -> #177 -> #178
#166 -> #182
#180 -> #184
#225 -> #226 -> #227
#228
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
| Google Search | #224 #225 #226 #227 #228 done |
