---
name: browser-project
description: Import of `~/dev/browser/CLAUDE.md`. Use when working in the browser repository to apply its commands, architecture map, testing expectations, issue-priority rule, and browser-specific review requirements.
---

Imported from `~/dev/browser/CLAUDE.md`.

Use this skill when the current task is in `/home/yunseong/dev/browser`.

## Project overview

Browser is a Rust browser engine and native UI app. The pipeline is:

`Network -> DOM -> Style -> Layout -> Render -> GUI`

## Common commands

```bash
cargo build
cargo run
cargo run --release
cargo test
cargo test <test_name>
cargo check
cargo clippy
```

## Architecture map

- `src/main.rs`: GUI app, async fetch/image orchestration, navigation state, JS runtime integration, full pipeline orchestration
- `src/dom.rs`: HTML parsing
- `src/css.rs`: CSS parser and selector specificity
- `src/style.rs`: styled tree, inheritance, inline style handling, stylesheet extraction
- `src/layout.rs`: layout tree and computed rectangles
- `src/render.rs`: raster painting
- `src/js.rs`: Boa-based JS runtime wrapper

## Project rules

- Use `./.agents/PRIORITY.md` to choose the next issue.
- Fixed render width is 800 px unless the repo changes that.
- For browser implementation work, `$browser-cli-reviewer` is the required live verification step before PR creation.
- For the broader developer workflow in this repo, combine this skill with `$developer`.
