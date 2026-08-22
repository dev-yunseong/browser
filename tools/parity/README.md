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

## The box model, and where it now stands

`LayoutBox::dimensions` used to be read as a **border box** in some places and a
**content box** in others, with no rule saying which. It now has one:

> An **auto** width or height leaves `dimensions` as the **border box**; a
> **stated** one leaves it as the **content box**.

That holds because both ways of arriving at an auto size — shrink-to-fit from
max-content, and a block filling `container_width - margins` — already count
padding and border in, while `border-box` sizing takes them back off a stated
one. Everything that reads `dimensions` follows the rule through one of three
helpers: `outer_width`/`outer_height` for the size a line box or flex line has
to reserve, `LayoutBox::paint_rect` for what a background and border cover, and
the `inner_width` computation for the width children are laid out against.

Getting there took three attempts, and the first two are worth knowing about.

The first migrated the width computation and painting together and left the flex
paths alone. Every test still passed and nearly every fixture got worse: flex
cards came out 30px too wide because the re-layout pass feeds an item's flexed
width back in as a containing-block width, and the padding is then taken off a
second time. The tests do not cover that interaction; only the pixel diff caught
it.

The second stopped taking padding off a *shrink-to-fit* width under
`border-box`, on the grounds that max-content already counts it. That fixes the
clipped button and immediately breaks `min-width`: the bounds were compared in
the content-box space the rest of the function assumed, so a `min-width: 85px`
box came out 24px too wide and a floated header cluster overflowed the viewport.

The third worked because the bounds were mapped into whichever space `width` is
held in, and because each reader was moved as it was found — with a probe
fixture measuring the result each time.

What is left is the flex algorithm's own convention: through it `dimensions`
holds the content box, which is what flex-basis and grow operate on, and it is
converted once at the end. That conversion is the one place where the two
meanings still meet.
