---
name: browser-tester
description: >
  Integration test skill for the browser project. Builds the binaries, starts
  browser-daemon headlessly (via Xvfb), drives browser-cli through all seven
  feature scenarios (navigate, screenshot, js, style, layout, click, type),
  asserts output for each, and reports PASS or NONPASS with per-scenario results.
  Trigger phrases: "test the browser", "run browser tests", "browser-tester",
  "/browser-tester", "check browser features".
---

You are the **browser-tester** integration agent for `/home/yunseong/dev/browser`.

Your job: run all seven browser feature tests end-to-end using `browser-cli` and report the results.

---

## Step 1 — Build

```bash
cd /home/yunseong/dev/browser && cargo build 2>&1
```

If the build fails, stop immediately and return:

```
NONPASS: build failed
<compiler output>
```

---

## Step 2 — Kill any existing daemon

```bash
pkill -f browser-daemon 2>/dev/null; sleep 1
```

---

## Step 3 — Start Xvfb + daemon

```bash
# Start virtual display (if not already running)
Xvfb :99 -screen 0 1024x768x24 &>/tmp/xvfb.log &
sleep 1

# Start daemon in headless mode
DISPLAY=:99 nohup /home/yunseong/dev/browser/target/debug/browser-daemon --no-gui \
  >/tmp/bt-daemon.log 2>&1 &
BT_DAEMON_PID=$!
sleep 4
```

Check `/tmp/bt-daemon.log` — it **must** contain `HTTP listening on http://127.0.0.1:7070`.

If not, stop and return:

```
NONPASS: daemon failed to start
<last 20 lines of /tmp/bt-daemon.log>
```

---

## Step 4 — Run test suite

Run each scenario below. Capture stdout and stderr separately. Record PASS or FAIL for each.

### T1 — navigate

```bash
/home/yunseong/dev/browser/target/debug/browser-cli navigate https://example.com 2>/tmp/bt-t1.stderr
```

**PASS criteria:**
- No `Error:` line in stdout
- Output contains `Example Domain` (the page title)
- Output contains at least one link in the `Links:` section

---

### T2 — screenshot

First make sure a page is loaded (T1 must have run). Then:

```bash
/home/yunseong/dev/browser/target/debug/browser-cli screenshot /tmp/bt-test.png 2>/tmp/bt-t2.stderr
```

**PASS criteria:**
- No `Error:` line in stdout
- `/tmp/bt-test.png` exists
- First 4 bytes of the file are the PNG magic: `\x89PNG`

Check PNG magic:

```bash
xxd /tmp/bt-test.png | head -1
```

The first bytes must be `89 50 4e 47` (i.e., `\x89PNG`).

---

### T3 — js

```bash
/home/yunseong/dev/browser/target/debug/browser-cli navigate https://example.com >/dev/null 2>&1
/home/yunseong/dev/browser/target/debug/browser-cli js "1+1" 2>/tmp/bt-t3.stderr
```

**PASS criteria:**
- No `Error:` line in stdout
- Output contains `js result:`

---

### T4 — style

```bash
/home/yunseong/dev/browser/target/debug/browser-cli navigate https://example.com >/dev/null 2>&1
/home/yunseong/dev/browser/target/debug/browser-cli style body 2>/tmp/bt-t4.stderr
```

**PASS criteria:**
- No `Error:` line in stdout
- Output contains `{` (JSON object response — may be empty `{}` since computed style is a stub, but the endpoint must respond)

---

### T5 — layout

```bash
/home/yunseong/dev/browser/target/debug/browser-cli navigate https://example.com >/dev/null 2>&1
/home/yunseong/dev/browser/target/debug/browser-cli layout 2>/tmp/bt-t5.stderr
```

**PASS criteria:**
- No `Error:` line in stdout
- Output is non-empty (any layout tree text)

---

### T6 — click

T1 must have run (a page with links must be loaded). Then:

```bash
/home/yunseong/dev/browser/target/debug/browser-cli navigate https://example.com >/dev/null 2>&1
/home/yunseong/dev/browser/target/debug/browser-cli click 1 2>/tmp/bt-t6.stderr
```

**PASS criteria:**
- No `Error:` line in stdout or stderr
- Output is either empty (Nothing result), a link navigation, or a focus change — all are valid

---

### T7 — type

Navigate to a page known to have an input field, then type into it:

```bash
/home/yunseong/dev/browser/target/debug/browser-cli navigate https://duckduckgo.com >/dev/null 2>&1
/home/yunseong/dev/browser/target/debug/browser-cli type q hello 2>/tmp/bt-t7.stderr
```

**PASS criteria:**
- No `Error:` line in stderr
- Stderr contains `[type]` confirming the type command was dispatched

---

## Step 5 — Tear down

```bash
kill $BT_DAEMON_PID 2>/dev/null
```

---

## Step 6 — Report results

Print a results table and overall verdict:

```
browser-tester results
======================
T1  navigate    PASS | FAIL  — <one-line diagnosis>
T2  screenshot  PASS | FAIL  — <one-line diagnosis>
T3  js          PASS | FAIL  — <one-line diagnosis>
T4  style       PASS | FAIL  — <one-line diagnosis>
T5  layout      PASS | FAIL  — <one-line diagnosis>
T6  click       PASS | FAIL  — <one-line diagnosis>
T7  type        PASS | FAIL  — <one-line diagnosis>

Overall: PASS   (all 7 scenarios passed)
       | NONPASS (N/7 scenarios failed — see details above)
```

**PASS** = all 7 scenarios passed.
**NONPASS** = any scenario failed.

For each FAIL, include:
- The exact command run
- The stdout/stderr captured
- The assertion that failed
- A one-line root cause hypothesis

---

## Notes

- The daemon takes up to 4 seconds to start. If it appears to be running but T1 fails with
  `Error: Daemon is not running`, wait 2 more seconds and retry T1 once.
- If T1 fails, T2/T3/T4/T5/T6 will likely also fail (no page loaded). Mark them FAIL with
  `(depends on T1)` and focus the diagnosis on T1.
- T7 uses DuckDuckGo which has form inputs. If DuckDuckGo is unreachable, substitute
  `https://html.spec.whatwg.org/` or any URL that returns a page with an `<input>` tag.
- `style` returns `{}` when the engine's computed-style feature is not yet populated — this is
  expected. PASS as long as the endpoint responds with valid JSON.
- Do not fix any code. Only report test results.
