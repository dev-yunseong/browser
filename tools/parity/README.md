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

This has been attempted once, migrating the width computation and painting
together and leaving the flex paths alone. Every test still passed and nearly
every fixture got worse: flex cards came out 30px too wide because the re-layout
pass feeds an item's flexed width back in as a containing-block width, and the
padding is then taken off a second time. The tests do not cover the interaction;
only the pixel diff caught it. A half-migration renders worse than either model
alone, so the next attempt needs the flex paths in the same change.

## Known limitation: inline runs do not fragment across lines

A text run is one box with one rect. When a run starts mid-line and wraps, the
continuation is drawn from that same rect's left edge rather than from the start
of the line box, so the second line of a paragraph containing inline elements is
indented by however far into the line the run began (`probe-inline`).

Fixing it means fragmenting an inline box into one rect per line it occupies,
which the line-building code does not currently model.
