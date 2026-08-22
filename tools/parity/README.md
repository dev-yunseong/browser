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

The visible cost left is a box with a *stated* width and padding painting its
padding short. Nested padding also fails to reduce the width passed to children,
so text can wrap later than it should.

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

The second attempt was narrower and did land, so the rule is now at least
statable:

> An **auto** width leaves `dimensions.width` as the **border box**; a **stated**
> width leaves it as the **content box**.

That holds because both ways of arriving at an auto width — shrink-to-fit from
max-content, and a block filling `container_width - margins` — already count
padding and border in, while `border-box` sizing takes them back off a stated
one. Before, an auto width under `border-box` had its padding taken off as well,
so a shrink-to-fit button came out exactly its own padding too narrow and its
label was drawn past the end of its background (`probe-controls`). `min-width`
and `max-width` are now mapped into whichever space `width` is being held in,
which is what the first version of this change got wrong: comparing a
content-box bound against a border-box width made a `min-width: 85px` box 24px
too wide and overflowed a floated header cluster.

The child content width now follows the same rule: a padded block lays its
children out against `width` less its padding when the width is auto, and
against `width` unchanged when it was stated. Before, a padded block handed its
children its own outer width and their text ran past its padding.

Paint reads `dimensions` as the border box, so what remains is the *stated*-width
case: a box with a declared width and padding paints its padding short. Fixing
that needs paint and the flex re-layout pass moved together — the first attempt
showed that changing one at a time renders worse than either model alone.

A flex item is the one other path that has been migrated, and only because its
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
