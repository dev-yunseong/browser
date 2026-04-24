---
name: browser-tester
description: Import of the browser project's Claude `browser-tester` skill. Use to run the browser project's seven end-to-end CLI scenarios and report PASS or NONPASS with per-scenario details.
---

Imported from `~/dev/browser/.claude/skills/browser-tester.md`.

Run from `/home/yunseong/dev/browser`.

## Goal

Build the browser, start `browser-daemon` headlessly, run seven CLI scenarios, and report a per-test result table plus an overall verdict.

## Scenarios

1. `navigate`
2. `screenshot`
3. `js`
4. `style`
5. `layout`
6. `click`
7. `type`

## Expectations

- Stop immediately if the build fails.
- Start a clean daemon instance and verify startup.
- Capture stdout and stderr for each scenario.
- For each failing scenario, report:
  - exact command
  - captured stdout/stderr
  - failed assertion
  - one-line root cause hypothesis

## Output

Produce a table like:

```text
browser-tester results
======================
T1  navigate    PASS | FAIL  — <diagnosis>
...

Overall: PASS
```

`PASS` requires all seven scenarios to pass.
