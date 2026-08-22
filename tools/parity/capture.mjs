#!/usr/bin/env node
/**
 * Capture Chromium reference material for visual-parity work.
 *
 * For every target site this writes three things into the output directory:
 *   <name>.reference.png   full-page Chromium screenshot at the project viewport
 *   <name>.viewport.png    first-viewport-only screenshot (easier to eyeball)
 *   <name>.html            a self-contained snapshot of the *rendered* DOM with
 *                          every stylesheet inlined and images embedded, so the
 *                          engine can be pointed at the exact same input
 *
 * The self-contained HTML is the important artefact: a screenshot alone cannot
 * be re-rendered, so without it a parity diff has nothing to diff against.
 *
 * Cross-origin stylesheets, images and web fonts are fetched through
 * Playwright's request context rather than from inside the page. A site that serves its CSS from a
 * separate asset domain — which is most large sites — exposes neither
 * `cssRules` nor a CORS-allowed `fetch` to page scripts, so an in-page fetch
 * yields a snapshot with no CSS at all, and a diff against it silently compares
 * two unstyled pages.
 *
 * Usage:
 *   npm --prefix tools/parity install
 *   node tools/parity/capture.mjs [outDir]
 */
import { chromium } from 'playwright';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

const VIEWPORT = { width: 800, height: 1200 };
const SETTLE_MS = 6000;
const NAV_TIMEOUT_MS = 60000;
const MAX_INLINE_BYTES = 400 * 1024;
// Fonts are the one asset worth a larger budget: a page measured with the wrong
// face breaks its lines somewhere else, so every line below the first is in the
// wrong place and the page comes out the wrong length.
const MAX_INLINE_FONT_BYTES = 3 * 1024 * 1024;

const TARGETS = [
  { name: 'github', url: 'https://github.com' },
  { name: 'naver', url: 'https://www.naver.com' },
  { name: 'yunseong', url: 'https://yunseong.dev' },
];

/** Rewrite relative `url()` references against the stylesheet's own URL. */
function absolutiseCss(cssText, baseHref) {
  if (!baseHref) return cssText;
  return cssText.replace(/url\((['"]?)([^'")]+)\1\)/g, (match, quote, ref) => {
    if (/^(data:|https?:|\/\/|#)/i.test(ref)) return match;
    try {
      return `url(${quote}${new URL(ref, baseHref).href}${quote})`;
    } catch {
      return match;
    }
  });
}

/**
 * Runs inside the page. Returns the serialised DOM plus the work Node has to
 * finish: stylesheets and images it could not read itself.
 */
function collect() {
  const sheets = [];
  for (const sheet of Array.from(document.styleSheets)) {
    let rules = null;
    try {
      rules = sheet.cssRules;
    } catch {
      rules = null;
    }
    if (rules) {
      sheets.push({ css: Array.from(rules).map((r) => r.cssText).join('\n'), href: sheet.href });
    } else if (sheet.href) {
      // Unreadable from here: cross-origin without CORS. Node fetches it.
      sheets.push({ href: sheet.href });
    }
  }

  const doc = document.documentElement.cloneNode(true);
  // Scripts are dropped on purpose: the snapshot is of the post-script DOM, so
  // re-running them would mutate an already-settled tree.
  doc.querySelectorAll('script, link[rel~="stylesheet"], link[rel="preload"], style').forEach((el) => el.remove());

  const cloned = Array.from(doc.querySelectorAll('img'));
  const live = Array.from(document.images);
  const images = [];
  cloned.forEach((img, i) => {
    img.removeAttribute('srcset');
    img.removeAttribute('loading');
    const src = live[i] && live[i].currentSrc;
    if (!src) return;
    img.setAttribute('data-capture-index', String(images.length));
    images.push(src);
  });

  return { html: doc.outerHTML, sheets, images, baseHref: location.href };
}

/** Fetch a URL through Playwright's request context, which ignores CORS. */
async function fetchAsset(request, url) {
  try {
    const res = await request.get(url, { timeout: 20000 });
    if (!res.ok()) return null;
    return await res.body();
  } catch {
    return null;
  }
}

async function capture(browser, target, outDir) {
  const context = await browser.newContext({
    viewport: VIEWPORT,
    deviceScaleFactor: 1,
    locale: 'ko-KR',
  });
  const page = await context.newPage();
  try {
    await page.goto(target.url, { waitUntil: 'load', timeout: NAV_TIMEOUT_MS });
    await page.waitForTimeout(SETTLE_MS);

    await page.screenshot({ path: path.join(outDir, `${target.name}.viewport.png`) });
    await page.screenshot({ path: path.join(outDir, `${target.name}.reference.png`), fullPage: true });

    const { html, sheets, images, baseHref } = await page.evaluate(collect);

    // Fill in the stylesheets the page could not read, keeping document order
    // so the cascade still resolves the way it did in the browser.
    let fetched = 0;
    const css = [];
    for (const sheet of sheets) {
      if (sheet.css !== undefined) {
        css.push(absolutiseCss(sheet.css, sheet.href));
        continue;
      }
      const body = await fetchAsset(context.request, sheet.href);
      if (body) {
        fetched += 1;
        css.push(absolutiseCss(body.toString('utf8'), sheet.href));
      }
    }

    // Inline the faces the page ships. A snapshot that still points at a CDN
    // renders with whatever fallback the machine happens to have, which is a
    // difference in the snapshot rather than in either renderer.
    let fontsInlined = 0;
    let fontBytes = 0;
    const fontCache = new Map();
    const inlineFonts = async (cssText) => {
      const urls = [...cssText.matchAll(/url\((['"]?)([^'")]+)\1\)/g)]
        .map((m) => m[2])
        .filter((u) => /\.(woff2?|ttf|otf)(\?|#|$)/i.test(u) && !u.startsWith('data:'));
      for (const url of new Set(urls)) {
        if (fontCache.has(url)) continue;
        const body = await fetchAsset(context.request, url);
        if (!body || body.length > MAX_INLINE_FONT_BYTES) {
          fontCache.set(url, null);
          continue;
        }
        const type = /\.woff2/i.test(url) ? 'font/woff2'
          : /\.woff/i.test(url) ? 'font/woff'
          : /\.otf/i.test(url) ? 'font/otf'
          : 'font/ttf';
        fontCache.set(url, `data:${type};base64,${body.toString('base64')}`);
        fontsInlined += 1;
        fontBytes += body.length;
      }
      return cssText.replace(/url\((['"]?)([^'")]+)\1\)/g, (match, quote, ref) => {
        const uri = fontCache.get(ref);
        return uri ? `url(${quote}${uri}${quote})` : match;
      });
    };
    for (let i = 0; i < css.length; i += 1) {
      if (css[i].includes('@font-face')) css[i] = await inlineFonts(css[i]);
    }

    const dataUris = await Promise.all(
      images.map(async (src) => {
        const url = new URL(src, baseHref).href;
        if (url.startsWith('data:')) return url;
        const body = await fetchAsset(context.request, url);
        if (!body || body.length > MAX_INLINE_BYTES) return url;
        const type = /\.svg(\?|$)/i.test(url) ? 'image/svg+xml' : 'image/png';
        return `data:${type};base64,${body.toString('base64')}`;
      }),
    );

    // Substitute by the index stamped on each img during collection.
    const withImages = html.replace(
      /<img\b[^>]*\bdata-capture-index="(\d+)"[^>]*>/g,
      (tag, index) => {
        const uri = dataUris[Number(index)];
        return uri ? tag.replace(/\bsrc="[^"]*"/, `src="${uri.replace(/"/g, '&quot;')}"`) : tag;
      },
    );

    const head = `<base href="${baseHref}"><style>${css.join('\n')}</style>`;
    const out = withImages.includes('<head>')
      ? withImages.replace('<head>', `<head>${head}`)
      : `<head>${head}</head>${withImages}`;

    await writeFile(path.join(outDir, `${target.name}.html`), `<!DOCTYPE html>\n${out}`, 'utf8');

    const cssBytes = css.reduce((n, c) => n + c.length, 0);
    console.log(
      `ok   ${target.name.padEnd(9)} ${(out.length / 1024).toFixed(0)} KB snapshot, ` +
        `${(cssBytes / 1024).toFixed(0)} KB CSS (${fetched} sheet(s) fetched server-side), ` +
        `${fontsInlined} font(s) inlined (${(fontBytes / 1024).toFixed(0)} KB)`,
    );
    if (cssBytes === 0) {
      console.error(`WARN ${target.name}: no CSS captured — the snapshot will render unstyled`);
    }
  } catch (err) {
    console.error(`FAIL ${target.name.padEnd(9)} ${err.message}`);
  } finally {
    await context.close();
  }
}

const outDir = path.resolve(process.argv[2] || 'tools/parity/captures');
await mkdir(outDir, { recursive: true });
const browser = await chromium.launch();
try {
  for (const target of TARGETS) await capture(browser, target, outDir);
} finally {
  await browser.close();
}
console.log(`\nWrote captures to ${outDir}`);
