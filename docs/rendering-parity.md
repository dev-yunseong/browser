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
| Grid and flex containers measured from their content edge | That counts the bottom padding but drops the top one, so every padded container came out exactly its `padding-top` short. A page built from padded entries ended hundreds of pixels short. |
| A padded flex item counted twice in its line | The line reserved the item's outer size by adding padding on top of `dimensions`, which for an auto-sized item already covers it. |
| No automatic minimum size on flex items | `min-width: auto` resolves to min-content, so shrinking never squeezes a box below its longest word. Without that floor a row of tags came out with each tag broken across two lines. |
| A text run given half the line | The caller subtracted what the line already held, and the run subtracted it again from its own start position. Every run after an inline sibling was laid out in half the room it had. |
| A line broken before its first word | The leading inter-element space counted as content, so the first word of a run could wrap onto a line of its own. |
| Collapsed inter-run space dropped when measuring | The intrinsic measurement trimmed the whitespace layout keeps, so a box sized from max-content was narrower than the run it had to hold and wrapped inside itself. |
| Flex containers measured as flow containers | A row's max-content took the *max* of its block-level children instead of their sum, so a "field + button" row was sized to whichever half was wider and the other hung outside it. The `gap` between items was left out of both intrinsic measurements as well, and the whitespace between two tags was counted as an item — a two-item row became four, and paid the gap twice more than it should have. |
| `box-sizing: border-box` honoured in layout but not in measurement | A stated width had its padding added on top again, and a stated *height* was never inset at all, so a `height: 48px` button drew 62px tall and pushed its row down. |
| Out-of-flow children counted toward intrinsic width | An absolutely positioned box is sized against its containing block and adds nothing to its parent's content width. github floats its "Enter your email" label over the field with `position: absolute`; counting it made the field 105px wider than the field. |
| Cascade layers unmodelled | An unlayered declaration beats every layer, and layers apply in the order they are named. github writes `.CtaFormControl-input::placeholder { opacity: 0 }` unlayered and lets it beat the `@layer primer-brand` rule 700KB later in the same page. |
| `::placeholder` unrecognised | A page that replaces the placeholder with its own floating label hides it with `opacity: 0`; the engine drew both, on top of each other. |
| Parent/child margins dropped rather than transferred | A first child's top margin and a last child's bottom margin *become* the parent's, which is what makes the space land outside the parent. Dropping them pulled the top of every page up by its first paragraph's margin. |
| Negative margins thrown away by the collapse | Two adjoining margins collapse to the largest positive plus the most negative; taking the plain maximum meant a negative margin met by zero simply vanished, and a page that overlaps two sections on purpose got a gap where the overlap should be. |
| CSS logical properties unrecognised | `padding-inline`, `margin-block`, `inset-inline-start`, `inline-size` and the rest were dropped outright. github's marketing header states its 24px side padding as `padding-inline` alone, so the header ran edge to edge and pushed "Sign in" off the viewport. |
| `top` and `bottom` together did not stretch a box | An overlay written as `position: absolute; inset: 0` came out its content's height — zero, for the empty element a gradient wash is — and never painted. |
| Percentage heights unresolved, percentage widths applied twice | `height: 50%` resolved to nothing; a positioned child's `width: 40%` was resolved against its own already-resolved width, giving 40% of 40%. |
| `filter: blur()` unparsed | A design system's decorative washes are gradients under a heavy blur, drawn as hard-edged blobs without it. The box blur behind it was wrong too — it seeded the accumulator with one window and slid it as if centred on another, so a blurred square came out as two bright bands with a hole between them. |
| `border-radius` in percent read as pixels | `border-radius: 50%` on a wide box is a full ellipse; reading the 50 as a pixel count drew a barely-rounded rectangle. |
| Broken images painted as a grey slab | A browser leaves the box transparent, outlines it and writes the `alt`. Filling it with light grey turned every unreachable image into the loudest thing on the page. |
| An `<svg>` with no `viewBox` had no intrinsic ratio | Its `width` and `height` attributes *are* its intrinsic size. github ships `<svg width="2280" height="1200">` as a spacer, and getting it wrong made that block 112px too tall. |
| Font stacks read as a set, not a list | Matching walks the stack in order and takes the first family the system can satisfy. Asking only whether the stack mentions `serif` or `monospace` anywhere sent every modern stack to the sans default — and `system-ui`, which yunseong.dev's stack reaches first, resolves to a face markedly wider than `sans-serif`. Every line of that page was measured in the wrong face. |
| `ch` and `ex` measured in the default face | They are metrics of the element's *own* font, so a `max-width: 46ch` cap came out 59px narrower than the browser's. |
| The space before a word left out of the fit test | A line could end up a space wider than its container — one line's difference in the height of any paragraph whose last word misses by less than a space. Paint already did it the right way, which is exactly why the two disagreed. |
| Kerning not applied | Advances were summed without it, so a DejaVu Sans line measured about a pixel wider than the browser draws it. |
| `white-space: pre-line` ignored | The source's own newlines were re-flowed away. |
| Grid items always stretched | They fill their cell only under `stretch`. `align-items: center` is how a page puts a portrait beside a taller column of text without letting the portrait grow to match it. |
| Bold threshold at 600 | CSS font matching with two weights takes the heavier face for any desired weight *above 500*, so `font-weight: 560` came out at book weight. |
| Only the first line of a run indented — for every line | A run after an inline element was measured against the leftover width for all of its lines, not just the first, so any paragraph with an inline element in it wrapped early. github's section headings — a bold lead-in span followed by the rest of the sentence — came out a line taller than the browser draws them, once per heading. |
| A stated height lost on a flex or grid container | Both paths derived a height from their content whenever `dimensions.height` had not been set yet, which is always. A `display: flex` button with `height: 48px` came out however tall its label made it. |
| A form control identified by its `display` | Primer sets `display: flex` on its text inputs; reading the display instead of the element collapsed github's email field to its own top padding. |
| A `var()` in a border shorthand placed by position alone | `border: solid var(--borderWidth-thin) transparent` has its only open slot at the *end*, and the width was dropped, leaving the button with no border at all. |
| The `background` shorthand did not reset the colour | Chromium serialises `background: none` back as `background: 0px 0px`, and github's accordions ship exactly that to turn off the UA's grey button fill — which stayed on, as a light bar behind every heading. |

## Where it stands

Measured with `node tools/parity/diff.mjs`, as a 16px block-mean difference
over the whole page, over the first fold's pixels, and over the whole page's
pixels:

| fixture | layout | fold | page | height (chromium -> engine) |
|---|---|---|---|---|
| github.com | 2.56% | 3.37% | 12.51% | 10570 -> 10654 |
| yunseong.dev | 1.51% | 3.34% | 3.99% | 4976 -> 5012 |
| naver.com | 1.32% | 2.89% | 2.56% | 18658 -> 16384 |

Sixteen of the twenty probe fixtures sit at or below 1%, and `probe-grid` is
pixel-identical over the fold. github.com began this run at 9.14% / 13.84% and
780px too tall; yunseong.dev at 4.24% and 256px too short.

naver.com's number means little: its snapshot was captured without CSS (see
below), so both renderers are drawing an unstyled page and the comparison only
exercises the UA stylesheet.

## What still limits parity

- **Canvas, video and script-driven content do not render.** github.com's
  landing page is largely a WebGL canvas and a video, so a large share of its
  remaining difference is content this engine does not draw at all rather than
  draws wrongly.
- **`filter: blur()` applies to an element's own background, not to its
  subtree.** The decorative washes it exists for are empty boxes, so that is
  the whole effect there; a filtered element with content of its own keeps that
  content sharp.
- **`justify-items` other than `stretch` is not modelled** — a grid item always
  fills its cell on the inline axis.
- **`transform` is not applied to painted boxes**, so a wash the page rotates
  or offsets sits square and where it was written.
- **Two box models coexist** — see `tools/parity/README.md` for the two failed
  migrations and what each one hit.
- **Inline runs do not fragment across lines.** A run's box spans the line box
  and its first line is indented, which is enough for line *breaking* to match;
  a wrapped run still cannot have a different height or background per line.
- **A snapshot can only be as good as its capture.** `capture.mjs` now inlines
  fonts as well as stylesheets and images, but a snapshot taken before that
  still points at a CDN, and a page rendered against a fallback face is a
  difference in the snapshot rather than in either renderer. naver.com's
  snapshot has no CSS in it at all and cannot be re-captured from a sandbox
  with no outbound network; re-capture before reading its numbers or a
  font-heavy page's.
