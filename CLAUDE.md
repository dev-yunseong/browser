# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Browser** — a web browser built from scratch in Rust. It fetches HTML/CSS from the network, parses and lays out the document, and renders it as a texture inside an `eframe`/`egui` native window.

## Commands

```bash
# Build
cargo build

# Run the browser
cargo run

# Run with release optimizations
cargo run --release

# Run tests
cargo test

# Run a single test
cargo test <test_name>

# Check for compile errors without building
cargo check

# Lint
cargo clippy
```

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
| `render.rs` | Walks the `LayoutBox` tree and paints into a `tiny_skia::Pixmap`. Handles text (via `ab_glyph`), background colors, borders, and images. |
| `js.rs` | Wraps `boa_engine` (`Context`). `JsRuntime::new()` installs mock globals (`window`, `document`, `navigator`, `console.log`). `execute()` runs script strings, silently skipping `import.meta` and suppressing repetitive DOM errors. |

### Key data-flow details

1. **Fetch** (`fetch_and_process`): `reqwest::blocking::get` fetches HTML; external CSS `<link>` hrefs are fetched and concatenated before parsing.
2. **Async image loading**: After the initial render, image URLs are enqueued as `poll_promise` threads. When they resolve they are inserted into `image_cache` and `trigger_re_render` is called.
3. **Hit-testing**: `current_links`, `current_form_controls`, and `current_event_handlers` are `Vec<(layout::Rect, String)>` stored on `BrowserApp`. On click, the pointer position relative to the texture rect is compared against each stored rect.
4. **JS execution**: Scripts are extracted from the DOM on page load, executed once in order by `JsRuntime`. `onclick` attribute handlers are stored in `LayoutBox::event_handlers` and re-executed on click events.

### Fixed viewport width

The render width is hardcoded to **800 px** (`src/main.rs:155`). Height is computed from layout (clamped 600–16384 px).

## Key dependencies

- `eframe`/`egui` — native GUI window and immediate-mode UI
- `html5ever` + `markup5ever_rcdom` — spec-compliant HTML5 parser
- `tiny-skia` — CPU-side 2D rasterizer (Pixmap)
- `ab_glyph` — TrueType font rasterization
- `boa_engine` — pure-Rust JavaScript interpreter
- `reqwest` (blocking) — HTTP/HTTPS fetching
- `poll-promise` — thread-based async promises for `egui`
- Assets: `assets/fonts/NanumGothic.ttf` embedded at compile time via `include_bytes!`
