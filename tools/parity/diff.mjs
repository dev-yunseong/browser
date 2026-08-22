#!/usr/bin/env node
/**
 * Diff this engine's raster output against Chromium for the same page.
 *
 * Fixtures are served over localhost so both renderers fetch byte-identical
 * input; anything the diff reports is therefore renderer drift, not content
 * drift. Chromium is driven through Playwright, this engine through the HTTP
 * control API that `browser-daemon --no-gui` exposes.
 *
 * Usage:
 *   node tools/parity/diff.mjs [name...]     # default: every fixture
 */
import { chromium } from 'playwright';
import { PNG } from 'pngjs';
import pixelmatch from 'pixelmatch';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { readFile, readdir, mkdir, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '../..');
const FIXTURES = path.join(HERE, 'fixtures');
const CAPTURES = path.join(HERE, 'captures');
const OUT = path.join(HERE, 'out');
const DAEMON = path.join(ROOT, 'target/release/browser-daemon');

const WIDTH = 800;
const VIEWPORT_HEIGHT = 1200;
const SERVE_PORT = 8099;
const DAEMON_PORT = 7071;
const CHROME = '/opt/pw-browsers/chromium';
const SETTLE_ATTEMPTS = 8;
const SETTLE_INTERVAL_MS = 1500;

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.json': 'application/json',
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function serveFixtures() {
  const server = createServer(async (req, res) => {
    const url = new URL(req.url, 'http://localhost');
    let rel = decodeURIComponent(url.pathname);
    if (rel.endsWith('/')) rel += 'index.html';
    const file = path.join(HERE, rel);
    if (!file.startsWith(FIXTURES) && !file.startsWith(CAPTURES)) {
      res.writeHead(403).end();
      return;
    }
    try {
      const body = await readFile(file);
      res.writeHead(200, { 'content-type': MIME[path.extname(file)] || 'application/octet-stream' });
      res.end(body);
    } catch {
      res.writeHead(404, { 'content-type': 'text/plain' }).end('not found');
    }
  });
  return new Promise((resolve) => server.listen(SERVE_PORT, '127.0.0.1', () => resolve(server)));
}

async function daemonUp() {
  try {
    const res = await fetch(`http://127.0.0.1:${DAEMON_PORT}/page`);
    return res.ok || res.status < 500;
  } catch {
    return false;
  }
}

async function startDaemon() {
  if (await daemonUp()) return null;
  if (!existsSync(DAEMON)) throw new Error(`missing ${DAEMON} — run: cargo build --release --bins`);
  const proc = spawn(DAEMON, ['--no-gui', '--port', String(DAEMON_PORT)], {
    stdio: ['ignore', 'ignore', 'ignore'],
    detached: false,
  });
  for (let i = 0; i < 60; i += 1) {
    await sleep(500);
    if (await daemonUp()) return proc;
  }
  proc.kill();
  throw new Error('browser-daemon did not become ready');
}

/** Chromium reference raster for one fixture URL. */
async function renderChromium(browser, url) {
  const context = await browser.newContext({
    viewport: { width: WIDTH, height: VIEWPORT_HEIGHT },
    deviceScaleFactor: 1,
  });
  const page = await context.newPage();
  await page.goto(url, { waitUntil: 'load', timeout: 30000 });
  await page.waitForTimeout(500);
  const buf = await page.screenshot({ fullPage: true });
  await context.close();
  return PNG.sync.read(buf);
}

/**
 * This engine's raster for the same URL, via the daemon control API.
 *
 * Images are fetched after the first layout and trigger a re-render, so a
 * screenshot taken straight after navigating can catch the page mid-load and
 * makes the comparison non-reproducible. Shots are repeated until two in a row
 * agree, which is the settled page a reader would see.
 */
async function renderEngine(url) {
  const shoot = async () => {
    const res = await fetch(`http://127.0.0.1:${DAEMON_PORT}/screenshot`);
    if (!res.ok) throw new Error(`screenshot failed: ${res.status}`);
    return Buffer.from(await res.arrayBuffer());
  };

  const nav = await fetch(`http://127.0.0.1:${DAEMON_PORT}/navigate`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ url }),
  });
  if (!nav.ok) throw new Error(`navigate failed: ${nav.status}`);

  let previous = await shoot();
  for (let attempt = 0; attempt < SETTLE_ATTEMPTS; attempt += 1) {
    await sleep(SETTLE_INTERVAL_MS);
    const current = await shoot();
    if (current.equals(previous)) return PNG.sync.read(current);
    previous = current;
  }
  console.warn(`  ${url}: engine output still changing after ${SETTLE_ATTEMPTS} shots`);
  return PNG.sync.read(previous);
}

/**
 * Mean colour difference over coarse blocks, as a fraction of full scale.
 *
 * The exact-pixel ratio is dominated by text: this engine and Chromium do not
 * ship the same faces, so every glyph differs at the pixel level even when the
 * layout is identical. Averaging over blocks keeps that noise bounded while
 * staying sensitive to what parity work is actually about — boxes in the wrong
 * place, wrong size, or the wrong colour.
 */
function blockDiff(a, b, width, height, block = 16) {
  const cols = Math.ceil(width / block);
  const rows = Math.ceil(height / block);
  let total = 0;
  for (let by = 0; by < rows; by += 1) {
    for (let bx = 0; bx < cols; bx += 1) {
      const sums = [0, 0, 0, 0, 0, 0];
      let count = 0;
      for (let y = by * block; y < Math.min((by + 1) * block, height); y += 1) {
        for (let x = bx * block; x < Math.min((bx + 1) * block, width); x += 1) {
          const i = (width * y + x) << 2;
          sums[0] += a.data[i];
          sums[1] += a.data[i + 1];
          sums[2] += a.data[i + 2];
          sums[3] += b.data[i];
          sums[4] += b.data[i + 1];
          sums[5] += b.data[i + 2];
          count += 1;
        }
      }
      if (!count) continue;
      for (let c = 0; c < 3; c += 1) {
        total += Math.abs(sums[c] - sums[c + 3]) / count;
      }
    }
  }
  return total / (rows * cols * 3 * 255);
}

/**
 * Pad a raster to `height` with white so two rasters of different page heights
 * can still be compared row-for-row. Height drift is reported separately rather
 * than being allowed to smear the pixel ratio.
 */
function pad(src, width, height) {
  const out = new PNG({ width, height });
  out.data.fill(0xff);
  for (let y = 0; y < Math.min(src.height, height); y += 1) {
    for (let x = 0; x < Math.min(src.width, width); x += 1) {
      const s = (src.width * y + x) << 2;
      const d = (width * y + x) << 2;
      out.data[d] = src.data[s];
      out.data[d + 1] = src.data[s + 1];
      out.data[d + 2] = src.data[s + 2];
      out.data[d + 3] = 0xff;
    }
  }
  return out;
}

/**
 * Captured snapshots are single self-contained files; hand-written fixtures are
 * directories with their own assets. Both are served from the same root.
 */
function fixtureUrl(name) {
  const capture = path.join(CAPTURES, `${name}.html`);
  const rel = existsSync(capture) ? `captures/${name}.html` : `fixtures/${name}/index.html`;
  return `http://127.0.0.1:${SERVE_PORT}/${rel}`;
}

async function compare(name, browser) {
  const url = fixtureUrl(name);
  const reference = await renderChromium(browser, url);
  const engine = await renderEngine(url);

  const height = Math.max(reference.height, engine.height);
  const a = pad(reference, WIDTH, height);
  const b = pad(engine, WIDTH, height);
  const diff = new PNG({ width: WIDTH, height });
  const mismatched = pixelmatch(a.data, b.data, diff.data, WIDTH, height, { threshold: 0.1 });

  // The first viewport is what a reader actually sees, so score it separately
  // from the whole page — a long page can hide a broken header in the average.
  const foldHeight = Math.min(VIEWPORT_HEIGHT, height);
  const foldA = pad(reference, WIDTH, foldHeight);
  const foldB = pad(engine, WIDTH, foldHeight);
  const foldMismatched = pixelmatch(foldA.data, foldB.data, null, WIDTH, foldHeight, { threshold: 0.1 });

  await mkdir(OUT, { recursive: true });
  await writeFile(path.join(OUT, `${name}.reference.png`), PNG.sync.write(reference));
  await writeFile(path.join(OUT, `${name}.engine.png`), PNG.sync.write(engine));
  await writeFile(path.join(OUT, `${name}.diff.png`), PNG.sync.write(diff));

  return {
    name,
    referenceHeight: reference.height,
    engineHeight: engine.height,
    page: mismatched / (WIDTH * height),
    fold: foldMismatched / (WIDTH * foldHeight),
    layout: blockDiff(foldA, foldB, WIDTH, foldHeight),
  };
}

const requested = process.argv.slice(2);
const names = requested.length
  ? requested
  : [
      ...(await readdir(CAPTURES).catch(() => []))
        .filter((f) => f.endsWith('.html'))
        .map((f) => f.replace(/\.html$/, '')),
      ...(await readdir(FIXTURES, { withFileTypes: true }).catch(() => []))
        .filter((e) => e.isDirectory())
        .map((e) => e.name),
    ].sort();

const server = await serveFixtures();
const daemon = await startDaemon();
const browser = await chromium.launch({ executablePath: CHROME });
const results = [];
try {
  for (const name of names) {
    try {
      results.push(await compare(name, browser));
    } catch (err) {
      results.push({ name, error: err.message });
    }
  }
} finally {
  await browser.close();
  server.close();
  if (daemon) daemon.kill();
}

const pct = (v) => `${(v * 100).toFixed(2)}%`;
console.log('\nfixture          layout   fold-px   page-px   height (chromium -> engine)');
console.log('-'.repeat(78));
for (const r of results) {
  if (r.error) {
    console.log(`${r.name.padEnd(15)} ERROR  ${r.error}`);
    continue;
  }
  const drift = r.engineHeight - r.referenceHeight;
  console.log(
    `${r.name.padEnd(15)} ${pct(r.layout).padStart(7)}  ${pct(r.fold).padStart(8)}  ${pct(r.page).padStart(8)}   ` +
      `${r.referenceHeight} -> ${r.engineHeight} (${drift >= 0 ? '+' : ''}${drift})`,
  );
}
console.log(`\nRasters in ${OUT}`);
