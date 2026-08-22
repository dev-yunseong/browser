# Visual parity tooling

Chromium is the reference renderer. These scripts capture what Chromium does
with a page and compare it against what this engine does with the *same* input.

## 1. Capture Chromium reference material

Run this on a machine with unrestricted network access:

```sh
npm --prefix tools/parity install
npx --yes playwright install chromium
node tools/parity/capture.mjs tools/parity/captures
```

For each target site this writes:

| File | What it is |
|---|---|
| `<name>.reference.png` | full-page Chromium screenshot at 800px width |
| `<name>.viewport.png` | first 800x1200 viewport only |
| `<name>.html` | self-contained snapshot of the rendered DOM, all CSS inlined |

The HTML snapshot is the load-bearing artefact. A screenshot alone cannot be
re-rendered, so parity work needs the exact input the reference was produced
from — otherwise a diff is measuring content drift, not renderer drift.

## 2. Diff the engine against the reference

```sh
node tools/parity/diff.mjs
```

This serves the snapshots over localhost, drives `browser-daemon` to screenshot
each one, and reports the per-site mismatched-pixel ratio.

## Fixtures

`fixtures/` holds hand-written probes that isolate one renderer feature each, so
a regression points at a mechanism rather than at "github.com looks wrong":

| Fixture | Covers |
|---|---|
| `smoke` | block/inline flow, flex row, borders, radii |
| `probe-hiding` | every idiom real sites use to hide content |
| `probe-sizing` | grid track sizing, flex basis, percentage widths |
| `probe-spacing` | margins and paddings from `var()` and `calc()`, grid and flex gaps |
| `probe-vars` | custom properties: fallbacks, arithmetic, theming by attribute |
| `probe-fixed` | `position: fixed`/`absolute`/`sticky`, out-of-flow heroes |
| `probe-gradient` | linear and radial gradients, stops outside the box |
| `probe-image` | intrinsic sizing, `object-fit`, data URIs |
| `probe-inline` | inline runs, reserved inter-element space, wrapping |
| `probe-overflow` | `overflow` clipping, `clip-path`, scroll containers |
| `probe-controls` | buttons, inputs and anchors styled as controls |
| `probe-flexitems` | inline children of a flex container, an inline flex container |
| `probe-navbar` | a real site's brand bar, markup and stylesheet taken verbatim |
| `probe-generics` | which face `sans-serif`, `serif` and `monospace` resolve to |
| `probe-webfont` | `@font-face` in woff2, woff and truetype, and family fallback |

## Known limitation: two box models in one engine

`LayoutBox::dimensions` is read as a **border box** in some places and a
**content box** in others, and the two readings are both load-bearing:

- Painting draws backgrounds and borders straight over `dimensions`, and the
  flex algorithm distributes free space into it — both of which only look right
  if it is the border box.
- `border_box_width()` adds padding and border *on top of* `dimensions`, and the
  child content width is taken as `dimensions` unchanged — both of which only
  hold if it is the content box.

The visible cost is a shrink-to-fit control coming out narrower than its own
label: `fit-content` already counts the padding, the border-box adjustment takes
it off again, and the label is clipped (`probe-controls`). Nested padding also
fails to reduce the width passed to children.

Fixing it means picking one meaning and migrating every reader — the width
computation, `border_box_width`/`margin_box_width`, the flex and grid track
code, the flex re-layout pass that re-runs an item at its flexed main size, and
the rect collectors — in one change, with `test_button_coordinate_collection`
and `test_border_box_min_size_includes_padding` updated to the chosen model.

This has been attempted twice, and both attempts are worth knowing about.

The first migrated the width computation and painting together and left the flex
paths alone. Every test still passed and nearly every fixture got worse: flex
cards came out 30px too wide because the re-layout pass feeds an item's flexed
width back in as a containing-block width, and the padding is then taken off a
second time. The tests do not cover that interaction; only the pixel diff caught
it.

The second tried the smallest possible version: stop taking padding off a
*shrink-to-fit* width under `border-box`, on the grounds that `max-content`
already counts it, so the painted box would cover the label. That does fix the
clipped button — and immediately breaks `min-width`. The bounds are compared
against `width` in the content-box space the rest of the function assumes, so a
`min-width: 85px` box came out 85px of *content* plus its padding, 24px too wide,
and a floated header cluster overflowed the viewport. Mapping the bounds into the
other space fixes that pair and breaks the next one along.

Both attempts say the same thing: the meaning of `dimensions` cannot be changed
for one path at a time. The next attempt needs the width computation, the
min/max bounds, the flex re-layout pass and paint in a single change, with
`test_button_coordinate_collection` and `test_border_box_min_size_includes_padding`
rewritten to the chosen model.

A flex item is the one path that has been migrated, and only because its
conversion could be done in one place: through the flex algorithm `dimensions`
holds the content box, which is what flex-basis and grow operate on, and it is
converted to the border box once, after the algorithm finishes.

## Known limitation: inline runs do not fragment across lines

A text run is one box with one rect. When a run starts mid-line and wraps, the
continuation is drawn from that same rect's left edge rather than from the start
of the line box, so the second line of a paragraph containing inline elements is
indented by however far into the line the run began (`probe-inline`).

Fixing it means fragmenting an inline box into one rect per line it occupies,
which the line-building code does not currently model.
