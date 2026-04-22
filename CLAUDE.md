# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Browser** — a web browser built from scratch in Rust. It fetches HTML/CSS from the network, parses and lays out the document, and renders it as a texture inside an `eframe`/`egui` native window.

## Commands

```bash
# Build
cargo build

# Build all binaries (daemon + cli + browser)
cargo build --bins

# Run the browser (opens GUI window — blocks until closed)
cargo run

# Run with release optimizations
cargo run --release

# Check for compile errors without building
cargo check

# Lint
cargo clippy

# ── Tests ────────────────────────────────────────────────────────────────────

# Fast: unit tests only — no render, completes in <5s
cargo test --lib

# Integration tests for a single file — each render call takes ~200ms in debug
cargo test --test test_pipeline

# Full suite — slow (~3+ min in debug due to render cost per test); always wrap
# with timeout to prevent session hang
timeout 300s cargo test -- --test-threads=2

# Single test by name
cargo test <test_name>

# Skip the heavy perf tests (each takes 6+ seconds)
cargo test -- --skip test_large_css --skip test_render_time_scaling

# ── Manual CLI testing (headless, no GUI, no Xvfb) ──────────────────────────

# Step 1: Build on host (uses local cargo cache — fast)
cargo build --bins

# Step 2: Run daemon inside drun rust:latest (resource-limited, compatible env)
drun --network host rust:latest ./target/debug/browser-daemon --no-gui --port 7070 &

# Step 3: Navigate — also inside drun rust:latest
sleep 4 && drun --network host rust:latest timeout 30s ./target/debug/browser-cli navigate https://yunseong.dev
```

## Execution Safety Rules

**Always build on host, run risky processes inside `drun rust:latest`.**

- Build on host: uses local `~/.cargo` cache — fast, incremental
- Run inside `drun rust:latest`: same Rust/Debian environment the binary was compiled for, plus hard resource caps (4 GB RAM / 0.5 CPU / 100 pids) — a hang or OOM kills the container cleanly, not the session

`drun` = `docker run --rm -v $(pwd):/app -w /app --memory=4g --cpus=0.5 --pids-limit 100`
Installed by the SessionStart hook at `~/.local/bin/drun`.

| Situation | Safe command |
|---|---|
| Compile / check / lint | `cargo build`, `cargo check`, `cargo clippy` — host only, safe |
| Unit tests (no render) | `cargo test --lib` — host only, fast (<5s) |
| Integration tests (render) | `timeout 300s cargo test -- --test-threads=2` — timeout required |
| Run full browser (GUI) | **Never run directly** — GUI event loop blocks forever |
| Run daemon headless | `cargo build --bins && drun --network host rust:latest ./target/debug/browser-daemon --no-gui` |
| Run CLI navigate | `drun --network host rust:latest timeout 30s ./target/debug/browser-cli navigate <url>` |

**Why drun for execution**: each render allocates a large Pixmap (~800×N×4 bytes). A hang or OOM in the render path kills the Docker container cleanly instead of freezing the session.

## Architecture

The pipeline runs: **Network → DOM → Style → Layout → Render → GUI**

### Module responsibilities (`src/`)

| File | Role |
|---|---|
| `main.rs` | `eframe::App` impl (`BrowserApp`). Owns the GUI event loop, navigation history, async fetch promises (`poll_promise`), image cache, form state, and JS runtime. Orchestrates the full pipeline in `process_html_with_cache`. |
| `dom.rs` | Thin wrapper around `html5ever` + `markup5ever_rcdom`. `parse_html()` returns an `RcDom`. |
| `css.rs` | Custom CSS parser. Defines `Value`, `Unit`, `Color`, `Selector`, `Rule`, `Stylesheet`. Supports `px`/`vw`/`vh`/`em` units and color keywords. Selector specificity computed as `(id, class, tag)`. |
| `style.rs` | Builds a `StyledNode` tree from a DOM `Handle` and a `Stylesheet`. Handles CSS inheritance (`color`, `font-size`, `font-family`), selector matching, and inline `style` attributes. Extracts `<style>` blocks and `<link rel=stylesheet>` hrefs. |
| `layout.rs` | Converts a `StyledNode` tree into a `LayoutBox` tree with computed `Rect` positions. Supports `Block`, `Inline`, `InlineBlock`, `ListItem`, `Table`/`TableRow`/`TableCell`, `Input`, and `Image` display types. Returns `(layout_tree, _, final_y)` from `build_layout_tree`. |
| `layer_tree.rs` | Builds a `PaintCommand` list from the `LayoutBox` tree. Emits `PushClip`/`PopClip` commands for boxes with `overflow: hidden` (with optional `border-radius` rounding). |
| `render.rs` | Walks the `LayoutBox` tree and paints into a `tiny_skia::Pixmap`. Handles text (via `ab_glyph`), background colors, borders, images, and clip masks (`overflow: hidden`). |
| `js.rs` | Wraps `boa_engine` (`Context`). `JsRuntime::new()` installs mock globals (`window`, `document`, `navigator`, `console.log`). `execute()` runs script strings, silently skipping `import.meta` and suppressing repetitive DOM errors. |

### Key data-flow details

1. **Fetch** (`fetch_and_process`): `reqwest::blocking::get` fetches HTML; external CSS `<link>` hrefs are fetched and concatenated before parsing.
2. **Async image loading**: After the initial render, image URLs are enqueued as `poll_promise` threads. When they resolve they are inserted into `image_cache` and `trigger_re_render` is called.
3. **Hit-testing**: `current_links`, `current_form_controls`, and `current_event_handlers` are `Vec<(layout::Rect, String)>` stored on `BrowserApp`. On click, the pointer position relative to the texture rect is compared against each stored rect.
4. **JS execution**: Scripts are extracted from the DOM on page load, executed once in order by `JsRuntime`. `onclick` attribute handlers are stored in `LayoutBox::event_handlers` and re-executed on click events.

### Fixed viewport width

The render width is hardcoded to **800 px** (`src/main.rs:155`). Height is computed from layout (clamped 600–16384 px).

## CLI Review (browser-cli-reviewer agent)

The developer agent **must always** invoke the `browser-cli-reviewer` agent before creating a PR — for every issue, regardless of which files changed. `cargo test` and `cargo clippy` verify code correctness only; the CLI reviewer verifies the browser actually runs and renders pages correctly.

The `browser-cli-reviewer` agent:
- Runs `cargo build --bins` to build fresh binaries
- Kills any existing daemon containers (`docker ps -q --filter "ancestor=rust:latest" | xargs docker kill`)
- Starts `./target/debug/browser-daemon --no-gui --port 7070` inside `drun rust:latest` (headless — no GUI)
- Runs `./target/debug/browser-cli navigate <url>` with `timeout 30s` against **both** `https://yunseong.dev` and `https://google.com`
- **For visual/rendering issues**: after navigation, also takes a screenshot by hitting `GET http://127.0.0.1:7070/screenshot`, saves the PNG, and uses the `Read` tool to visually inspect it — checking for layout bugs, misaligned elements, broken flex/grid, missing content. If visual defects are found, fix them and repeat before creating the PR.
- Returns **PASS** or **NONPASS** with full output and diagnosis
If the reviewer returns **NONPASS**, fix the issue and re-run the reviewer before proceeding to PR.

## Issue Priority

When choosing which issue to work on next, always consult **`./.agents/PRIORITY.md`**.
It defines the canonical priority order and dependency graph for all open issues.
Pick the highest-priority open issue that has no unresolved dependencies.

## Key dependencies

- `eframe`/`egui` — native GUI window and immediate-mode UI
- `html5ever` + `markup5ever_rcdom` — spec-compliant HTML5 parser
- `tiny-skia` — CPU-side 2D rasterizer (Pixmap)
- `ab_glyph` — TrueType font rasterization
- `boa_engine` — pure-Rust JavaScript interpreter
- `reqwest` (blocking) — HTTP/HTTPS fetching
- `poll-promise` — thread-based async promises for `egui`
- Assets: `assets/fonts/NanumGothic.ttf` embedded at compile time via `include_bytes!`
