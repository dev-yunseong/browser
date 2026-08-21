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
