# Browser

**Browser**는 Vibe Coding으로 밑바닥부터 하나씩 쌓아 올린 웹 브라우저입니다. Rust의 강력함과 egui의 유연함을 결합하여, 네트워크 통신부터 HTML/CSS 파싱, 레이아웃 엔진, 그리고 JavaScript 실행까지 브라우저의 핵심 파이프라인을 직접 구현했습니다.

A browser built from scratch through Vibe Coding. It's not just an application; it's a journey into the heart of the web.

## Core Tech Stack
- **Language:** Rust (Edition 2021)
- **GUI & Windowing:** `eframe` / `egui` (Immediate-mode GUI framework)
- **HTML Parsing:** `html5ever`, `markup5ever_rcdom` (Spec-compliant HTML5 parser)
- **CSS Parsing:** Custom implementation based on `cssparser`
- **2D Rendering:** `tiny-skia` (High-performance CPU-based rasterizer)
- **JavaScript Engine:** `boa_engine` (Pure-Rust JS interpreter)
- **Networking:** `reqwest` (Blocking mode)
- **Font Rendering:** `ab_glyph`

## How to Run

Running in **release mode** is strongly recommended for smooth rendering.

```bash
# GUI browser (default)
cargo run --release

# Headless daemon only (HTTP API on port 7070)
cargo run --release --bin browser-daemon -- --no-gui

# CLI client (interactive REPL — requires daemon running)
cargo run --release --bin browser-cli
```

## Components

### GUI Browser

The default binary (`cargo run`) opens a native window powered by `eframe`/`egui`. Type a URL in the address bar and press Enter to load a page.

### browser-daemon

A background process that exposes the browser engine over HTTP on `localhost:7070`. Other tools (scripts, the CLI, automation) talk to it via a simple REST API.

| Endpoint | Method | Description |
|---|---|---|
| `/navigate` | POST `{ "url": "..." }` | Load a URL, returns page state |
| `/page` | GET | Current page state (title, markdown, links, forms) |
| `/click` | POST `{ "x": N, "y": N }` | Simulate a click at coordinates |
| `/type` | POST `{ "text": "..." }` | Type text into the focused input |
| `/submit` | POST | Submit the current form |
| `/screenshot` | GET | PNG screenshot of the current page |
| `/js` | POST `{ "script": "..." }` | Evaluate JavaScript |

Custom port: `browser-daemon --port 7071` or `BROWSER_DAEMON_PORT=7071`.

### browser-cli

An interactive terminal client that talks to `browser-daemon`. Renders pages as text with numbered links and form controls.

```
browser-cli — connecting to daemon on port 7070
Type 'help' for available commands, 'quit' to exit.

[browser] > navigate https://example.com

# Example Domain

This domain is for use in illustrative examples...

Links:
  [1] More information  →  https://www.iana.org/domains/reserved

[browser: example.com] > click 1
[browser: example.com] > screenshot page.png
```

**Available commands:**

| Command | Description |
|---|---|
| `navigate <url>` | Load a URL |
| `click <N>` | Click link by number |
| `click "<text>"` | Click link or button matching text |
| `type <field> <value>` | Type text into a named input |
| `select <field> <option>` | Choose a select option |
| `submit` | Submit the current form |
| `back` / `forward` | Navigate browser history |
| `screenshot [file]` | Save PNG (default: `screenshot.png`) |
| `logs` | Print browser console entries captured during page load |
| `tick [count]` | Advance daemon JavaScript tasks and re-render if needed |
| `help` | Show all commands |
| `quit` | Exit |

Command aliases: `nav`/`open`/`goto` → navigate, `b` → back, `f`/`fwd` → forward, `ss` → screenshot.

Single-command mode (no REPL): `browser-cli navigate https://example.com`

Custom port: `browser-cli --port 7071` or `BROWSER_DAEMON_PORT=7071`.

## Features
- **Network:** Fetches HTML and external CSS/image resources via `reqwest`.
- **DOM:** Parses HTML into a DOM tree using `html5ever`.
- **Style:** Combines DOM and CSS rules into a styled node tree.
- **Layout:** Computes block, inline, and inline-block layout from the style tree.
- **Render:** Paints final pixels with `tiny-skia` and displays them as an `egui` texture.
- **JS Runtime:** Integrates `boa_engine` for basic JavaScript execution.

Enjoy the vibe of the web. 🚀
