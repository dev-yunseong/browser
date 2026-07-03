# Naver Playwright Parity Check

Use this check for #257 and follow-up Naver visual parity work. It captures a Chromium baseline and the browser-daemon output at the project viewport width.

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

Start the daemon in one terminal:

```bash
timeout 180s ./target/debug/browser-daemon --no-gui --port 7071
```

Then run:

```bash
timeout 75s ./target/debug/browser-cli --port 7071 navigate https://www.naver.com
timeout 30s ./target/debug/browser-cli --port 7071 logs
timeout 45s ./target/debug/browser-cli --port 7071 screenshot /tmp/browser-naver-parity/naver-browser.png
```

## Expected Result

- `naver-playwright.png` and `naver-browser.png` both exist.
- Browser output shows populated Naver shell content beyond raw placeholders such as `#shortcutArea`, `#root`, and `#footer`.
- `browser-cli logs` has no critical startup blocker such as `Cannot read properties of undefined (reading 'onsubmit')`.
- Any visible layout/CSS drift is recorded in the PR or a linked follow-up issue. As of #257, remaining Naver layout/CSS drift is tracked in #288.
