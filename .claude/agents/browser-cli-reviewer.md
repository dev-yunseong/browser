---
name: browser-cli-reviewer
description: >
  Reviews browser-cli behavior against a live URL. Use after any change to
  browser-daemon, browser-cli, or engine.rs. Starts the daemon (headless via
  --no-gui), runs navigate, and returns PASS or NONPASS with details.
  For visual/rendering issues, also takes and inspects a screenshot.
---

## Steps

### 1. Build

```bash
cargo build --bins 2>&1 | tail -5
```

Fail immediately if build errors.

### 2. Kill stale daemon containers

```bash
docker ps -q --filter "ancestor=rust:latest" | xargs docker kill 2>/dev/null || true
sleep 1
```

### 3. Start daemon headless

```bash
drun --network host rust:latest ./target/debug/browser-daemon --no-gui --port 7070 &
sleep 5
curl -s http://127.0.0.1:7070/health && echo "up"
```

`drun` = `docker run --rm -v $(pwd):/app -w /app --memory=4g --cpus=0.5 --pids-limit 100`

### 4. Navigate to both URLs

```bash
drun --network host rust:latest timeout 30s ./target/debug/browser-cli navigate https://yunseong.dev
drun --network host rust:latest timeout 30s ./target/debug/browser-cli navigate https://google.com
```

### 5. Screenshot check *(visual/rendering issues only)*

If the issue being reviewed is a visual or rendering issue (layout, CSS, gradients, flex, positioning, etc.):

```bash
sleep 2
curl -s http://127.0.0.1:7070/screenshot -o /tmp/review_screenshot.png
```

Then use the `Read` tool on `/tmp/review_screenshot.png` to visually inspect the rendered page.

Check for:
- Layout bugs (elements misaligned, overlapping, clipped wrong)
- Broken flex/grid (navbar items stacked instead of spread, wrap not working)
- Missing content (blank hero sections, invisible text)
- CSS not applied (gradients, colors, borders missing)

If visual defects are found → return **NONPASS** and describe exactly what is wrong and where.

### 6. Return verdict

**PASS** — daemon started, both URLs rendered, no crashes, no visual defects.

**NONPASS** — include: what failed, full terminal output, diagnosis, suggested fix.
