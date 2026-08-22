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
| `::before` and `::after` modelled as text runs | Layout sized a generated box by its text, so `content: ""` came out nothing at all and a stated `width`, `height`, `background` or `position` was never read. That is most of what generated content does: the disc behind github's play button, the stripe down a card, the wash behind a hero. |
| A selector's verdict cached across elements that share a signature | Only ancestor combinators and attribute constraints opted out of the cache; pseudo-classes did not. The first `.row:first-child` verdict was handed to every `.row` on the page — yunseong.dev's project list gave each of its five entries the first one's zero top padding and came out 128px short. |
| Attribute-selector operators read as plain equality | `[class^="…"]`, `[class$=…]`, `[class*=…]`, `[class~=…]` and `[class|=…]` all left the operator stuck on the attribute's *name*, so none of them matched anything. github styles its whole link component through `[class^="Primer_Brand__Link-module__Link___"]`. |
| A pseudo-class argument split on its own spaces | `:not(.a + .a)` was torn into three parts by the pass that spaces out combinators, so the rule matched nothing. github's logo strip lost its 32px of top padding to it. |
| `aspect-ratio` applied to the content box under `border-box` | The ratio sizes whichever box `box-sizing` names. github's hero frame — a `1000 / 1196` box inside 8px of padding and a 1px border — came out 21px short in each of four sections. |
| A ratio-settled height not treated as definite | `height: 100%` inside a ratio-sized frame resolved to `auto`, so the media that fills such a frame never painted. |
| The ratio not re-applied after flexing | Growing or shrinking an item along the main axis moves the other axis with it; re-laying the item out reads its own stated width back, which is the one flexing just overruled. |
| `grid-template-rows: 0fr` read as content-sized | A zero flex factor takes none of the free space and contributes no content, which is how every modern accordion holds a panel closed. Reading the track as content-sized left every collapsed panel standing open — ~300px per section on github. |
| `flex: unset` dropped | The shorthand knew only `none`, `auto` and numbers, so the `flex: 1 1 0%` an author wrote `flex: unset` to undo stayed in force and github's security hero split its row evenly instead of 65/35. |
| `rem` inside `calc()` resolved against the element | Layout has the element's font size but not the root's and passed the former for both, so `calc(100% - 2 * 2rem)` came out 56px of inset where the page asks for 64. |
| A nested cascade layer ranked by its qualified name | `base.inner` sorts at `base`'s place among the top-level layers, not after every layer mentioned before it — so a rule in `base.inner` beat one in `components`, backwards. |
| `mask-image` unsupported | How a page hides the hard edge of a background it lays over a section. github's hero carousel drew its two backing gradients at full strength from the top of the section instead of fading them in. |
| `mix-blend-mode` unsupported | A glow set to `plus-lighter` over a dark section reads as light, not as paint; composited normally it comes out about half as bright as the page intends. |
| Gradients interpolated in unpremultiplied space | CSS interpolates premultiplied, where a fully transparent stop contributes no colour: `#9a7cff` fading to `rgba(14,10,162,0)` stays purple the whole way. Interpolating the stated colours ringed every glow in dark blue. |
| A radial gradient's radii, stated outright, read as a colour stop | `radial-gradient(141.53% 114.68% at 87.46% 55.27%, …)` names no extent keyword, and falling back to a farthest-corner circle is a very different wash. |
| Out-of-flow children appended rather than kept in tree order | Paint order among positioned boxes is document order. github's hero carousel painted its video under the two gradients that sit behind it in the markup. |
| An escaping absolute box carried along with its flex item | A flex or grid item is laid out at the origin and offset into place afterwards; a descendant already placed in page coordinates moved twice. github's carousel put its video 52px right of where the page has it. |
| A broken image sized as a fixed placeholder | An image whose bytes never arrived *is* its alt text, laid out in the element's own box. Every screenshot on github's security pillars is remote, and reading them as a fixed box left each of those columns 38px short. |
| `border-radius` reduced to one value for the whole box | `border-radius: 24px 24px 0 0` rounds the top of a panel and leaves it flush at the bottom; the `/` syntax that gives each corner separate horizontal and vertical radii was thrown away outright. |
| `min-height` and `max-height` compared in the wrong box | The bound is a border box under `border-box` sizing while a measured height already is one, so insetting the bound and leaving the height alone compared a bound shorn of its padding against a height that still carried it. |
| An absolutely positioned child resolved against the content box | Its containing block is the ancestor's *padding* box (CSS 2.2 §10.1), so an `inset: 0` overlay came out short by the padding it exists to cover. |
| `background: var(--x)` never reaching paint | The shorthand holds the reference and the colour slot holds the reset the shorthand emits; substitution happens long after the shorthand was split, and the result was never put where paint looks for it. |
| `overflow: hidden` never reaching the text inside it | A text run carries its own clip rect and never consults the mask stack the clip is pushed onto, so a clipped panel's prose painted straight over whatever followed it. A disclosure held shut at `grid-template-rows: 0fr` spilled its whole panel across the heading below. |
| An empty clip rect read as no clip at all | `Rect::from_xywh` rejects a rect that is zero along an axis, and the rejection was handled as a failed allocation — fall back to painting unclipped. That is the exact inverse of an empty clip, which paints nothing. |
| Every `<img>` wrapped by a freshly created, detached one | `HTMLImageElement`'s constructor was the legacy `new Image(w, h)`, which *makes* an element rather than adopting the one the node factory hands it. Script reading an image on the page saw no `src`, no `class` and no parent, and each traversal leaked another stray `<img>` into the document. |
| `getBoundingClientRect` reporting the content box | It read `dimensions` straight, which holds the content box on an axis a stated value settled. A control at `height: 40px` under `border-box` sizing answered 22 — its height shorn of the padding and border the property includes. |
| A generated box answering for a real element | `::before` and `::after` are synthesised elements with no parent, so all of them key to the empty document path, and whichever went in last answered for every element whose own key could not be built. |
| `:has()` unsupported | The pseudo-class a modern design system reacts to its own contents with. github's hero pads itself only when it holds a UI panel — `.lp-SectionHero-visual:has(.…--copilotUI) { padding: 48px 96px }` — and without it the section came out 31px short, which is most of the page's whole height deficit. |
| Taking a box out of flow did not blockify it | Every generated box this engine builds is a `<span>`, so `::before { content: ""; position: absolute; inset: 0 }` — a wash over a card, a glow behind a panel, the disc behind an icon — was laid out inline and came out its content's size, which for an empty generated box is nothing at all. |
| An absolute box under a static parent sized against a containing block with no height yet | The block belongs to an ancestor whose own height is not settled until its in-flow children are, and the descendant was placed before that. `height: 100%` came out zero. github's hero glow is a `::before` sized exactly that way, under a static wrapper inside a `position: relative` carousel. |
| Every positioned box treated as a stacking context | `position: relative` with `z-index: auto` is not one, so a `z-index: -1` child of it belongs further up and paints *below* that box's own background. Painting it as an ordinary negative child put github's hero glow on top of the panel it sits behind. |
| Only the last `::before` rule carrying `content` applied | A design system states the shape once and overrides a size or a colour in a later, equally specific rule that names no `content` of its own. Those overrides were thrown away outright. |
| An intrinsic width measured without the box's own `min-width` | What a box contributes to its parent's intrinsic size is its content bounded by its own constraints. github's hero toggle is five buttons at `min-width: 110px` around shorter labels; measured from the labels the row came out 109px short, and its `overflow: hidden` clipped the last button away. |
| Whitespace at the edge of a block's content measured as a space | CSS drops it, and the intrinsic measurement did not. A date written as two spans inside a `<p>` indented in the source carries a whitespace-only text node on each side; counting those made the box three spaces wider than the run it holds, and on yunseong.dev that was enough to push the heading beside it onto a second line. |
| `order` ignored by grid auto-placement | Flex honoured it; grid placed its items in plain document order. github alternates its feature sections by giving one column `order: 2` and the other `order: 1`, so every screenshot came out on the side the text belongs on, and the text on the picture's side. |
| An `overflow: hidden` clip never reaching a descendant *layer* | A clip is expressed as PushClip/PopClip inside one layer's own command list, and a box that composites on its own — positioned, transformed or blended — never sees it. That only shows on a box whose paint spreads past its own bounds: github's hero glow is blurred by 42px, and its halo escaped the intro section and washed over the whole band below it. |
| `filter` in a `style` attribute dropped | The inline-style parser had no case for it, so an inline blur never produced the longhand paint reads and the box came out sharp. |
| Cascade layer order ranked below specificity | Layer order beats specificity outright: an unlayered rule wins over one in any layer however specific that one is. Re-ordering the source so unlayered rules came last only settled *ties*; github states its component rules inside `@layer primer-brand` and overrides them with plain page-level classes, so a four-class `:not()` selector kept its 24px margin over the unlayered rule's 16px and every pillar came out 8px too tall. |
| A character no bundled face covers never looked at the system's fonts | A browser resolves a family it cannot satisfy through the system's own fonts, and the fallback for an uncovered codepoint is the same search. This engine went straight to the bundled NanumGothic, whose Hangul advance is 0.94em against the 1.00em of the Unifont Chromium picks here, so every Korean run on yunseong.dev came out 5.5% narrow — enough to keep a line the reference wraps. |
| An atomic inline aligned to the line's top instead of its baseline | Everything on a line hangs from one baseline, and an empty `inline-block` — the twelve-pixel square an icon is — rests its bottom margin edge there. Top-aligning it put every icon beside a run of text three pixels too high. |
| A block's bottom margin dropped before inline content | The margin is held back to collapse with the *next block's* top margin; inline content after it forms an anonymous block, which has none to collapse with, so the held margin is simply space before it. Held and never spent, it vanished. |
| The baseline placed a fixed 0.85em below the line's top | It sits half the *leading* plus the font's ascent below it, which is the split layout already measured its line boxes with. Ignoring the leading drew every run on a line taller than its own font that much too high — three pixels at the `line-height: 1.5` a design system writes, ten at `line-height: 40px` on a 16px face. |
| `text-decoration` never reaching the text it underlines | It does not inherit, but the line an ancestor draws runs under its in-flow descendants — and the text node carrying the glyphs is where the line is actually drawn. Nothing carried it there, so no link on any page was underlined. |
| A comment counted as content between two blocks | It took an empty inline box, which broke the two blocks' margins collapsing: a comment written between a heading and a paragraph pushed the paragraph down by the heading's whole margin. |
| Flow advancing past the content box | A box that states a height and carries padding handed the next block a cursor its own padding too high, and every section below it climbed by that much. |


## Where it stands

Measured with `node tools/parity/diff.mjs`, as a 16px block-mean difference
over the whole page, over the first fold's pixels, and over the whole page's
pixels:

| fixture | layout | fold | page | height (chromium -> engine) |
|---|---|---|---|---|
| github.com | 1.20% | 2.84% | 1.55% | 10570 -> 10553 |
| yunseong.dev | 1.03% | 2.56% | 2.68% | 4976 -> 4976 |
| naver.com | 1.21% | 3.10% | 2.68% | 18658 -> 16384 |

github.com began this work at 9.14% layout, 13.84% fold and 780px too tall;
yunseong.dev at 4.24% and 256px too short. github's page is now within a single
pixel of Chromium's height, and only about a sixtieth of its pixels differ at
all. Its remaining vertical drift is around 16px, gained in one section and
carried down the page; every section's own boxes are within a pixel or two.

Every probe fixture sits at or below 1.7% on the layout metric, and
`probe-grid`, `probe-transform` and `probe-aspect` are pixel-identical over the
fold. `probe-has`, `probe-overlay`, `probe-layers` and `probe-grid` cover what this
round found. yunseong.dev's page is now exactly Chromium's height.

naver.com's number means little: its snapshot was captured without CSS (see
below), so both renderers are drawing an unstyled page and the comparison only
exercises the UA stylesheet.

## What still limits parity

- **Glyph rasterisation.** Most of what is left on a text-heavy fixture is not
  layout: converting both renders to greyscale removes only a twentieth of the
  difference, so it is glyph shape and hinting rather than the reference's
  subpixel antialiasing. Advance widths agree — across the whole of github.com
  only three text leaves differ in width by 4px or more, and across yunseong.dev
  only two — and both renderers position glyphs at quarter-pixel phases. What
  differs is that FreeType hints stems onto the pixel grid and this rasteriser
  does not. The text-coverage curve now brings the ink to within 4-10% of the
  reference's, from 6-13% short.
- **Canvas, video and script-driven content do not render.** github.com's
  landing page is largely a WebGL canvas and a video, so a share of its
  remaining difference is content this engine does not draw at all.
- **`filter: blur()` applies to an element's own background, not to its
  subtree.** The decorative washes it exists for are empty boxes, so that is
  the whole effect there; a filtered element with content of its own keeps that
  content sharp. The blur's falloff is also a three-pass box blur rather than a
  true Gaussian, which spreads a heavy blur's energy differently — github's
  hero glow reads about half as bright as Chromium's at the same distance.
- **`justify-items` other than `stretch` is not modelled** — a grid item always
  fills its cell on the inline axis.
- **Two box models coexist.** `dimensions` holds the content box on an axis a
  stated value settled and the border box on one that was measured;
  `LayoutBox` now records which, and everything outside layout goes through
  `outer_width`/`outer_height`. See `tools/parity/README.md` for the two failed
  migrations and what each one hit.
- **A deeper absolute descendant still resolves against the content box.** The
  padding-box rule is applied where a positioned ancestor places its own
  children; one further down still inherits the content box.
- **Inline runs do not fragment across lines.** A run's box spans the line box
  and its first line is indented, which is enough for line *breaking* to match;
  a wrapped run still cannot have a different height or background per line.
- **Transformed overflow does not extend the page.** A rotated or scaled box
  that reaches past the document's own bottom does not lengthen it, so
  `probe-transform` ends 59px short of Chromium's scroll height.
- **A named system family is not resolved.** `font-family: "DejaVu Sans"` or
  `"Liberation Sans"` falls through to the bundled sans rather than loading the
  file the system has, even though the fallback search now reads those same
  files. Generic families and the codepoint fallback both match Chromium
  exactly; only a family named outright does not.
- **A snapshot can only be as good as its capture.** `capture.mjs` now inlines
  fonts as well as stylesheets and images, but a snapshot taken before that
  still points at a CDN, and a page rendered against a fallback face is a
  difference in the snapshot rather than in either renderer. naver.com's
  snapshot has no CSS in it at all and cannot be re-captured from a sandbox
  with no outbound network; re-capture before reading its numbers or a
  font-heavy page's.
