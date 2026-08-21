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

/** This engine's raster for the same URL, via the daemon control API. */
async function renderEngine(url) {
  const nav = await fetch(`http://127.0.0.1:${DAEMON_PORT}/navigate`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ url }),
  });
  if (!nav.ok) throw new Error(`navigate failed: ${nav.status}`);
  const shot = await fetch(`http://127.0.0.1:${DAEMON_PORT}/screenshot`);
  if (!shot.ok) throw new Error(`screenshot failed: ${shot.status}`);
  return PNG.sync.read(Buffer.from(await shot.arrayBuffer()));
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
console.log('\nfixture         fold-diff   page-diff   height (chromium -> engine)');
console.log('-'.repeat(72));
for (const r of results) {
  if (r.error) {
    console.log(`${r.name.padEnd(15)} ERROR  ${r.error}`);
    continue;
  }
  const drift = r.engineHeight - r.referenceHeight;
  console.log(
    `${r.name.padEnd(15)} ${pct(r.fold).padStart(9)}   ${pct(r.page).padStart(9)}   ` +
      `${r.referenceHeight} -> ${r.engineHeight} (${drift >= 0 ? '+' : ''}${drift})`,
  );
}
console.log(`\nRasters in ${OUT}`);
