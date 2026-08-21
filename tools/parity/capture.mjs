#!/usr/bin/env node
/**
 * Capture Chromium reference material for visual-parity work.
 *
 * For every target site this writes three things into the output directory:
 *   <name>.reference.png   full-page Chromium screenshot at the project viewport
 *   <name>.viewport.png    first-viewport-only screenshot (easier to eyeball)
 *   <name>.html            a self-contained snapshot of the *rendered* DOM with
 *                          every stylesheet inlined and small images embedded,
 *                          so the engine can be pointed at the exact same input
 *
 * The self-contained HTML is the important artefact: a screenshot alone cannot
 * be re-rendered, so without it a parity diff has nothing to diff against.
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

const TARGETS = [
  { name: 'github', url: 'https://github.com' },
  { name: 'naver', url: 'https://www.naver.com' },
  { name: 'yunseong', url: 'https://yunseong.dev' },
];

/**
 * Runs inside the page. Serialises the live DOM with all CSS inlined.
 *
 * Same-origin sheets are read straight off `cssRules`; cross-origin sheets throw
 * on that access, so they are re-fetched by href and their relative url() refs
 * are absolutised against the sheet's own URL.
 */
async function serialise(maxInlineBytes) {
  const absolutise = (cssText, baseHref) => {
    if (!baseHref) return cssText;
    return cssText.replace(/url\((['"]?)([^'")]+)\1\)/g, (match, quote, ref) => {
      if (/^(data:|https?:|\/\/)/i.test(ref)) return match;
      try {
        return `url(${quote}${new URL(ref, baseHref).href}${quote})`;
      } catch {
        return match;
      }
    });
  };

  const sheets = [];
  for (const sheet of Array.from(document.styleSheets)) {
    let rules = null;
    try {
      rules = sheet.cssRules;
    } catch {
      rules = null;
    }
    if (rules) {
      sheets.push(absolutise(Array.from(rules).map((r) => r.cssText).join('\n'), sheet.href));
      continue;
    }
    if (!sheet.href) continue;
    try {
      const res = await fetch(sheet.href);
      if (res.ok) sheets.push(absolutise(await res.text(), sheet.href));
    } catch {
      /* unreachable sheet: skip, the diff will show it as missing style */
    }
  }

  const toDataUri = async (url) => {
    try {
      const res = await fetch(url);
      if (!res.ok) return null;
      const blob = await res.blob();
      if (blob.size > maxInlineBytes) return null;
      const buf = new Uint8Array(await blob.arrayBuffer());
      let binary = '';
      for (const byte of buf) binary += String.fromCharCode(byte);
      return `data:${blob.type || 'application/octet-stream'};base64,${btoa(binary)}`;
    } catch {
      return null;
    }
  };

  const doc = document.documentElement.cloneNode(true);

  // Scripts are dropped on purpose: the snapshot is of the post-script DOM, so
  // re-running them would mutate an already-settled tree.
  doc.querySelectorAll('script, link[rel~="stylesheet"], link[rel="preload"], style').forEach((el) => el.remove());

  const head = doc.querySelector('head') || doc.insertBefore(document.createElement('head'), doc.firstChild);
  const base = document.createElement('base');
  base.setAttribute('href', location.href);
  head.insertBefore(base, head.firstChild);
  const styleEl = document.createElement('style');
  styleEl.textContent = sheets.join('\n');
  head.appendChild(styleEl);

  const live = Array.from(document.images);
  const cloned = Array.from(doc.querySelectorAll('img'));
  for (let i = 0; i < cloned.length; i += 1) {
    const src = live[i] && live[i].currentSrc;
    cloned[i].removeAttribute('srcset');
    cloned[i].removeAttribute('loading');
    if (!src) continue;
    const data = await toDataUri(src);
    cloned[i].setAttribute('src', data || new URL(src, location.href).href);
  }

  return `<!DOCTYPE html>\n${doc.outerHTML}`;
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

    const html = await page.evaluate(serialise, MAX_INLINE_BYTES);
    await writeFile(path.join(outDir, `${target.name}.html`), html, 'utf8');

    console.log(`ok   ${target.name.padEnd(9)} ${(html.length / 1024).toFixed(0)} KB snapshot`);
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
