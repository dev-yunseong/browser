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
| `margin: 0 auto` centred the content width | A padded container sat half its padding off to one side and took its whole subtree with it. |
| `min-height` / `max-height` applied only in block layout | A navbar sized by `min-height` collapsed to its content and everything below it moved up. |
| Hairlines drawn at fractional offsets | A 1px `#161718` rule spread across two rows and came out `#656667`. On a design built from hairlines that is most of its visible structure. |
| `display: flex` parsed as a length | Introduced while adding `ch`/`ex`: the parser matched a unit suffix without checking that a number preceded it, and `flex` ends in `ex`. Caught by the probes the same day. |
| Custom property names lowercased | Property names are case-insensitive; custom ones are not. Every `var()` naming a mixed-case token — `--borderColor-default`, which is how a design system names all of them — looked up a property nothing had defined, so a whole palette resolved to nothing. |
| Selector lists split on every comma | `:has(p, div, pre)` carries commas of its own, so one narrow selector became several wide ones. A fragment as broad as `div` matched every division on the page and drew GitHub's focus ring around 639 elements. |
| Leading `var()` in a border shorthand read as the colour | The width then defaulted to `medium`, so `border-top: var(--borderWidth-thin) solid #fff9` came out as a 3px rule in the element's text colour — white, on a dark surface. |
| `#rgba` hex colours unparsed | A translucent tint fell back to the property's initial value. |
| Bold text measured with the regular face | A bold face is wider at the same size, so headings and brand marks measured short and the box behind them ended before the text did — the next item was then drawn on top of the last word. |
| `letter-spacing` ignored | Applied by neither measurement nor paint, so a tracked heading was the wrong width and broke in the wrong place. |
| Two of three generic families bound to the wrong face | The bundled `sans-serif` was right only by accident — the file named `DejaVuSans.ttf` was in fact Liberation Sans — while `serif` fell back to sans and the bold sans was 17% too wide. |
| `@font-face` discarded | A page designed around its own face was measured with a bundled one, so every line broke somewhere else. WOFF and WOFF2 are what every real site ships, so an engine that reads only bare TTF loads none of them. |
| Flex container one `gap` too tall | The trailing gap was only removed for multi-line containers, so every single-line flex row was one gap too tall — and a page built from stacked flex rows accumulated the error all the way down. |
| Shrink-wrapped flex item narrowed by its own margin | Max-content is a content width and knows nothing of margins, but the block sizing path subtracts them from whatever it is handed. An item with a side margin came out that much narrower than its content, and its children were shrunk to fit. |
| `align-items` ignored on a column flex container | Items were measured at the container's full width, so `center` had nothing to centre. |
| Gradient stops clamped to the box | `#fff 117%` means the gradient never reaches white inside the box; clamping made it reach white at the bottom edge, so a hero faded a whole shade too early. |
| UA block margins stated in pixels | `p`, `ul`, `h1`-`h6` take `em` margins in the spec's sheet, so they track the page's font size. Fixed pixels pinned a page's rhythm to a 16px body, and `h3`-`h6` had no margins at all. |
| Grid `auto` tracks sized from free space | `auto` and `fr` drew from the same pool, so on `auto 1fr` the auto track swallowed the row and the column beside it came out empty. |
| `inline-flex` unrecognised | A button built as `display: inline-flex` — which is how a design system builds every button — fell through to its tag's default and became an ordinary inline box, drawn the full width of its bar. |
| Shrink-to-fit boxes narrowed by their own padding | Max-content already counts padding in; taking it off again under `border-box` made a button exactly its own padding too narrow, so its label ran past the end of its background. |
| A padded inline child's box counted twice | Line boxes added padding and border on top of `dimensions`, which for an auto-sized box already covers them. A section holding a padded button came out 20px too tall, once per section down the page. |
| Form controls inherited the page's `line-height` | The UA sheet gives them a font of their own, which is why a button inside `body { line-height: 1.4 }` is not 1.4 lines tall. |
| Void elements serialised with an end tag | `</br>` is parsed as *another* `<br>`, and the serialised DOM is what the second render parses — so every page gained a blank line per line break each time it was re-rendered. |

## Where it stands

Thirteen of the fifteen probe fixtures sit at or near 1% of a 16px block-mean
diff, and `probe-gradient` is pixel-identical over the first fold. The two that
are not — `probe-controls` at 2.0% and `probe-inline` at 1.5% — are held back by
the box model and by inline fragmentation, both below. Of the three sites,
yunseong.dev is at 4.2% and github.com at 14.5%; naver.com cannot be measured
until its snapshot is re-captured with CSS in it.

## What still limits parity

- **Canvas, video and script-driven content do not render.** github.com's
  landing page is largely a WebGL canvas and a video, so a large share of its
  remaining difference is content this engine does not draw at all rather than
  draws wrongly.
- **Two box models coexist** — see `tools/parity/README.md` for the two failed
  migrations and what each one hit. The visible cost is a shrink-to-fit control
  coming out narrower than its own label (`probe-controls`).
- **Inline runs do not fragment across lines**, so a wrapped run's continuation
  is indented to wherever the run began (`probe-inline`).
- **A snapshot can only be as good as its capture.** `capture.mjs` now inlines
  fonts as well as stylesheets and images, but a snapshot taken before that
  still points at a CDN, and a page rendered against a fallback face is a
  difference in the snapshot rather than in either renderer. Re-capture before
  reading a font-heavy page's numbers.
