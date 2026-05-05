# Issue Priority Archive — Completed Sections

All sections below are fully resolved. See `PRIORITY.md` for active work.

---

## Priority 0 - Daemon / CLI Architecture ✓

| # | Issue | Why first |
|---|---|---|
| #68 | Extract Headless BrowserEngine ✓ | Decouples engine from GUI. |
| #70 | Browser Daemon (engine + GUI + HTTP server) ✓ | CLI and GUI share state. |
| #72 | Refactor BrowserApp as daemon GUI module ✓ | No engine logic in main.rs. |
| #69 | CLI HTTP client (Markdown + REPL) ✓ | Thin HTTP client to daemon. |
| #71 | Browser Tester Agent ✓ | Project-local skill. |

## Priority 1 - Layout Correctness ✓

| # | Issue | Why first |
|---|---|---|
| #13 | Intrinsic & Extrinsic Sizing ✓ | min-content/max-content for shrink-wrap. |

## Priority 2 - Compositor Pipeline ✓

| # | Issue |
|---|---|
| #30 | Layer Tree Builder ✓ |
| #32 | Compositor Transform & Matrix Engine ✓ |
| #31 | Tile & Texture Management ✓ |
| #33 | Layer Blending & Composition Loop ✓ |

## Priority 3 - Event Loop ✓

| # | Issue |
|---|---|
| #22 | Event Loop: Macro/Micro Task Integration ✓ |
| #38 | Render-step Orchestrator ✓ |
| #40 | Idle Callback Scheduler ✓ |
| #25 | Focus Management & Sequential Navigation ✓ |

## Priority 4 - Security Model ✓

| # | Issue |
|---|---|
| #27 | CORS Enforcement Layer ✓ |
| #28 | Origin-Based Storage & SOP ✓ |
| #29 | Content Security Policy Engine ✓ |

## Priority 5 - Parser ✓

| # | Issue |
|---|---|
| #19 | WHATWG-compliant HTML5 Tokenizer ✓ |

## Priority 6 - Engine Performance ✓

| # | Issue |
|---|---|
| #60 | Parallel Async CSS Fetching & Processing ✓ |
| #61 | Style Tree Memory Optimization (OOM) ✓ |
| #62 | CSS Parser & Selector Matching Opt ✓ |
| #63 | Layout Engine Recursion & Scalability ✓ |

## Priority 7 - Google.com Layout Fidelity ✓

| # | Issue |
|---|---|
| #103 | Layout fidelity umbrella ✓ |
| #110 | Form control intrinsic sizing ✓ |
| #111 | Absolute/fixed header positioning ✓ |
| #112 | Spacing and line-box alignment ✓ |
| #113 | Pre-layout DOM/runtime parity ✓ |
| #119 | History, text mutation, URL correctness ✓ |
| #124 | Hero/logo and search cluster ✓ |
| #127 | Search row control sizing ✓ |
| #126 | Stray right-edge overflow ✓ |
| #125 | Footer anchoring ✓ |
| #134 | Header utility cluster ✓ |
| #133 | Advanced search link ✓ |
| #132 | Search action button labels ✓ |
| #131 | Footer legal copy spacing ✓ |

## Priority 8 - GUI Render Stability ✓

| # | Issue |
|---|---|
| #137 | Viewport width stability ✓ |

## Priority 9 - CSS Engine Completeness ✓

| # | Issue |
|---|---|
| #143 | CSS custom properties ✓ |
| #142 | Basic flexbox row layout ✓ |
| #145 | border-radius on form controls ✓ |
| #144 | CSS transform ✓ |
| #150 | @media query parsing ✓ |
| #156 | prefers-color-scheme: dark ✓ |
| #157 | CSS Grid layout ✓ |
| #151 | box-shadow ✓ |
| #152 | list-style-type ✓ |
| #158 | ::before/::after pseudo-elements ✓ |

## Priority 10 - DevTools ✓

| # | Issue |
|---|---|
| #107 | Developer console panel ✓ |
| #108 | Console REPL ✓ |

## Priority 11 - DOM Core Completeness ✓

| # | Issue |
|---|---|
| #166 | Core node model umbrella ✓ |
| #167 | Selector unification ✓ |
| #168 | DOM mutation correctness ✓ |
| #169 | Serialization (innerHTML/outerHTML) ✓ |
| #170 | Event dispatch phases ✓ |
| #171 | Layout-backed geometry APIs ✓ |
| #172 | Collections and traversal ✓ |
| #173 | MutationObserver ✓ |
| #174 | Shadow DOM ✓ |
| #175 | Form DOM state parity ✓ |
| #185 | DocumentFragment semantics ✓ |
| #186 | Comment/DocumentType nodes ✓ |
| #187 | Sibling navigation APIs ✓ |
| #188 | Document surface semantics ✓ |
| #194 | cloneNode(deep) ✓ |
| #195 | Fragment insertion invariants ✓ |
| #196 | innerHTML replacement ✓ |
| #197 | Detached subtree adoption ✓ |
| #205 | HTMLCollection ✓ |
| #206 | TreeWalker ✓ |
| #207 | Range API ✓ |
| #208 | NodeIterator ✓ |

## Priority 12 - Browser JS / Web API Completeness ✓

| # | Issue |
|---|---|
| #176 | Web API umbrella ✓ |
| #177 | Fetch API ✓ |
| #178 | XMLHttpRequest ✓ |
| #179 | Cookie jar ✓ |
| #180 | URL/location API ✓ |
| #181 | History API ✓ |
| #182 | CSSOM JS surface ✓ |
| #183 | window/navigator/screen ✓ |
| #184 | Dynamic script loading ✓ |

## Priority 13 - Google Search (Form + Navigation) ✓

| # | Issue |
|---|---|
| #224 | Umbrella ✓ |
| #225 | Form metadata extraction ✓ |
| #226 | EngineCmd::Submit ✓ |
| #227 | Enter-key form submission ✓ |
| #228 | URL bar search fallback ✓ |
