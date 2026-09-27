# Naver Playwright Parity Check

Use this check for #257, #288 and follow-up Naver visual parity work. It captures a Chromium baseline and the browser-daemon output at the project viewport (800 x 1200).

## What each capture shows

- The Playwright capture is the 800 x 1200 **viewport** at the page's scroll position when the capture is taken. Naver's script calls `#query.focus()`, and Chromium scrolls the focused search box into view with the nearest edge, so the Chromium page is scrolled right by about 64px (`scrollX` = 64; the document is 1340px wide).
- `browser-cli screenshot --viewport` is the matching capture from this engine: the 800 x `--viewport-height` rectangle at the current document scroll offset. `position: fixed` boxes are painted at the viewport, not at the document origin.
- `browser-cli screenshot` (same as `--full-page`) is the whole page from the document origin, 800px wide. It stays the default, because fixture checks compare full-page captures.

## Capture Playwright Baseline

```bash
mkdir -p /tmp/browser-naver-parity
timeout 90s npx --yes playwright screenshot \
  --browser chromium \
  --viewport-size "800,1200" \
  --wait-for-timeout 5000 \
  https://www.naver.com \
  /tmp/browser-naver-parity/naver-playwright.png
```

## Capture Browser Output

Build first with `cargo build --release --bins`. Start the daemon in one terminal:

```bash
timeout 300s ./target/release/browser-daemon --no-gui --port 7071 --viewport-height 1200
```

Then run:

```bash
timeout 150s ./target/release/browser-cli --port 7071 navigate https://www.naver.com
sleep 8
timeout 60s ./target/release/browser-cli --port 7071 tick 20
timeout 30s ./target/release/browser-cli --port 7071 logs
# Compare this one with the Playwright capture:
timeout 60s ./target/release/browser-cli --port 7071 screenshot --viewport /tmp/browser-naver-parity/naver-browser-viewport.png
# Whole page from the document origin:
timeout 60s ./target/release/browser-cli --port 7071 screenshot /tmp/browser-naver-parity/naver-browser.png
```

The HTTP form is `GET /screenshot?mode=viewport` or `GET /screenshot?mode=full` (the default).

To check the scroll position the engine settled on:

```bash
timeout 30s ./target/release/browser-cli --port 7071 js "JSON.stringify([scrollX, scrollY, document.documentElement.scrollWidth, document.activeElement && document.activeElement.id])"
```

Chromium reports `[64,0,1340,"query"]` for the same page.

## Compare

```bash
python3 - <<'EOF'
from PIL import Image, ImageChops, ImageStat
a = Image.open('/tmp/browser-naver-parity/naver-playwright.png').convert('RGB')
b = Image.open('/tmp/browser-naver-parity/naver-browser-viewport.png').convert('RGB')
h = min(1200, a.height, b.height)
d = ImageStat.Stat(ImageChops.difference(a.crop((0, 0, 800, h)), b.crop((0, 0, 800, h)))).mean
print(f"mean_abs_diff={sum(d) / 3:.2f} over 800x{h}")
EOF
```

The live page changes between loads (ads, banners, news), so compare horizontal positions of the header, search box and newsstand rather than expecting a zero difference.

## Expected Result

- `naver-playwright.png` and `naver-browser-viewport.png` both exist and are 800 x 1200.
- The search box, header icons and newsstand line up horizontally between the two captures.
- Browser output shows populated Naver shell content beyond raw placeholders such as `#shortcutArea`, `#root`, and `#footer`.
- `browser-cli logs` has no critical startup blocker such as `Cannot read properties of undefined (reading 'onsubmit')`.
- Any visible layout/CSS drift is recorded in the PR or a linked follow-up issue. Remaining Naver layout/CSS drift is tracked in #288.
