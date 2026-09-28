// Capture URL with Playwright Chromium CAPTURE_AT seconds after navigation
// starts (the same clock scripts/parity/cap.sh uses), so time-driven content
// such as rolling banners lines up between the two captures.
//
// usage: node chromium_at.js URL OUT.png [CAPTURE_AT] [PROXY]
const { chromium } = require('playwright');
(async () => {
  const [url, out, at = '10', proxy] = process.argv.slice(2);
  const browser = await chromium.launch(proxy ? { proxy: { server: proxy } } : {});
  const page = await browser.newPage({ viewport: { width: 800, height: 1200 }, ignoreHTTPSErrors: true });
  const start = Date.now();
  await page.goto(url, { waitUntil: 'load', timeout: 60000 }).catch(() => {});
  const remaining = Number(at) * 1000 - (Date.now() - start);
  if (remaining > 0) await page.waitForTimeout(remaining);
  await page.screenshot({ path: out });
  await browser.close();
})();
