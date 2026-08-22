# Rendering parity: what was broken and how it was found

A Chromium-referenced pixel-diff harness (`tools/parity/`) was pointed at
snapshots of github.com, naver.com and yunseong.dev. Every defect below was
found by rendering the *same* local input in both engines and looking at where
they disagreed, then isolating the mechanism in a probe fixture.

## Method

Live sites are a bad reference: their content changes under you, and network
failures read as rendering failures. `capture.mjs` freezes a page instead — the
settled DOM with every stylesheet inlined and images embedded — so both
renderers get byte-identical input and any difference is renderer drift.

Cross-origin stylesheets are fetched through Playwright's request context, not
from inside the page. A page script can read neither `cssRules` nor a
CORS-allowed `fetch` for a stylesheet served from an asset domain, which is what
most large sites do; an in-page fetch yields a snapshot with no CSS at all, and
a diff against that silently compares two unstyled pages.

Each finding got a probe fixture that isolates one mechanism, so a regression
points at a cause rather than at "github.com looks wrong".

## Defects found

Ordered by how much of a page each one destroyed.

| Defect | Effect |
|---|---|
| CSS comments were never stripped | The parser splits on `{`, `}` and `;`, so a comment glued itself to the next selector and discarded that rule. On a hand-written stylesheet that is most of the sheet. |
| `@layer` / `@supports` bodies discarded | Only `@media` survived. Modern design systems put nearly everything inside a layer, so GitHub's design tokens vanished and its hero painted black on a dark background. |
| Cascade ties broken by hash order | Equal-specificity rules applied in whatever order the selector index yielded — the wrong winner, and not stable between runs. The same page measured three different heights in five renders. |
| `::before` / `::after` rules applied to their subject | `.lp-Intro::before { position: fixed }` took `.lp-Intro` itself out of flow; its `overflow: hidden` then clipped away the entire hero inside it. |
| Universal selector never matched | `* { box-sizing: border-box }` — which nearly every stylesheet opens with — did nothing, so padded boxes computed their width as if padding were outside them. GitHub laid out 174px wider than the viewport. |
| Unknown pseudo-classes never matched | `.collapse:not(.show) { display: none }` never applied, so Bootstrap's collapsed nav rendered expanded. |
| `calc()` / `clamp()` / `min()` / `max()` unparsed | Every property using one fell back to its initial value; a fluid type scale collapses to body size and the spacing rhythm goes with it. |
| `var()` inside `calc()` unparsed | `calc(var(--gutter) * .5)` is how a spacing scale derives one step from another, and how Bootstrap computes container padding. |
| `rem` treated as `em` | Resolved against the parent instead of the root, so a design system's scale drifted smaller the deeper it nested. |
| Borders modelled as one uniform edge | `border-bottom` was never expanded, so the hairline rules that carry a layout's structure did not exist. |
| `minmax(0, 1fr)` split on whitespace | A two-column template became three junk tracks; body text wrapped one word per line. |
| `grid-column: span N` ignored | Twelve-column systems put every item in a single cell a twelfth of its width. |
| Media queries: no range syntax, no compound conditions | `(width >= 768px)` and `(min-width: a) and (max-width: b)` never matched, dropping whole responsive layers. |
| `prefers-color-scheme` reported dark | A site's dark-mode layer painted dark surfaces under text coloured for a light one. |
| `display: block` on `<img>` | Read as "become a plain block", so the image was never painted and the box had no intrinsic height. |
| Scaled images drawn at position × scale | `draw_pixmap` transforms the placed result, so an image scaled to a tenth landed at a tenth of its y and painted over the content above it. |
| `visibility`, `clip`, `clip-path` unimplemented | Every screen-reader-only skip link painted over the page. |
| Numeric `font-weight` not read as bold | `font-weight: 700` — the form nearly every stylesheet uses — left headings at book weight. |
| `line-height` ignored | Line boxes were a fixed 1.4x the font size, so every page came out the wrong length. |
| One font for everything | Latin text was measured with a Korean face, so it wrapped in the wrong places and pages ran about a quarter short. |
| `ch` / `ex` unsupported | `max-width: 46ch` was dropped and prose ran the full width of its container. |
| Auto-offset out-of-flow boxes pinned to their containing block | A hero positioned below a header jumped to the top of the page. |
| Reserved inter-element space not drawn | Paint trimmed the run layout had reserved space for, so text after an inline element ran into it. |

## What still limits parity

- **Web fonts are not loaded.** `@font-face` is skipped, so a page designed
  around a specific face is measured with a bundled one. Advance widths differ,
  so lines break in different places and page heights differ by around a fifth.
  This is the largest remaining source of drift on a text-heavy page.
- **Canvas, video and script-driven content do not render.** github.com's
  landing page is largely a WebGL canvas and a video, so a large share of its
  remaining difference is content this engine does not draw at all rather than
  draws wrongly.
- **Two box models coexist** — see `tools/parity/README.md`. Shrink-to-fit
  controls come out narrower than their labels.
- **Inline runs do not fragment across lines**, so a wrapped run's continuation
  is indented to wherever the run began.
