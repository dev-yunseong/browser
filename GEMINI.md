# Browser (GEMINI.md)

This file provides the structure, configuration, and development conventions for the Browser project. Use this document to understand the context when collaborating with AI agents.

## Project Overview

**Browser** is a web browser being built from scratch using Rust. It aims to implement core browser pipelines directly—from network calls to HTML/CSS parsing, layout calculation, 2D rendering, and a JavaScript execution engine.

### Core Tech Stack
- **Language:** Rust (Edition 2021)
- **GUI & Windowing:** `eframe` / `egui` (Immediate-mode GUI framework)
- **HTML Parsing:** `html5ever`, `markup5ever_rcdom` (Spec-compliant HTML5 parser)
- **CSS Parsing:** Custom implementation based on `cssparser`
- **2D Rendering:** `tiny-skia` (High-performance CPU-based rasterizer)
- **JavaScript Engine:** `boa_engine` (Pure-Rust JS interpreter)
- **Networking:** `reqwest` (Using Blocking mode)
- **Font Rendering:** `ab_glyph`

### Architecture Pipeline
1.  **Network:** Fetch HTML and external CSS/Image resources via `reqwest`.
2.  **DOM:** Parse HTML with `html5ever` to create an `RcDom` tree (`src/dom.rs`).
3.  **Style:** Combine the DOM tree with parsed CSS rules to create a `StyledNode` tree. Includes inheritance and specificity calculations (`src/style.rs`, `src/css.rs`).
4.  **Layout:** Traverse the `StyledNode` tree to compute sizes and positions (Rects), creating a `LayoutBox` tree. Supports Block, Inline, Inline-Block, Flex, etc. (`src/layout.rs`).
5.  **Render:** Traverse the `LayoutBox` tree and draw to a Pixmap using `tiny-skia` (`src/render.rs`).
6.  **GUI:** Convert the final rendered Pixmap into an `egui` texture for display. Manages the address bar, navigation buttons, and history (`src/main.rs`).

## Building and Running

Use the following commands from the project root:

```bash
# Build the project
cargo build

# Run the browser
cargo run

# Run in optimized release mode (recommended for rendering performance)
cargo run --release

# Run tests
cargo test

# Code lint check
cargo clippy
```

## Mandatory Workflow

All development tasks MUST follow the **Plan-Review-Act-Verify** steps below:

1.  **Plan:** Formulate a detailed technical plan for the task.
2.  **Pre-Review:** Pass the plan to the 'Reviewer Agent' (`generalist` sub-agent) for critique.
3.  **Iteration:** Revise the plan based on feedback until the Reviewer declares **'Pass'**.
4.  **Development (Act):** Start actual code modification ONLY after explicit **'Pre-Review Pass'**.
5.  **Verification:** After implementation, invoke the 'Reviewer Agent' again to compare the original **Plan** with the actual **Code**.
6.  **Final Pass:** If the Reviewer provides feedback, revise the code (or plan) and repeat until a final **'Pass'** is granted.

### 1. Language Policy
- **Internal thoughts and code comments:** English
- **User-facing responses and documents:** Korean
- **Agent memory and instructions:** English

### 2. Code Quality & Stability
- **Error Handling:** Avoid `.unwrap()` in production code; handle `Result` or `Option` appropriately for stability.
- **Testing Required:** Always add automated test cases in the `tests/` directory when implementing new features or fixing bugs.
- **Viewport:** The rendering width is dynamic based on the window size, passing through the layout engine.

### 3. Key Module Roles
- `src/main.rs`: `eframe::App` implementation, event loop, and async resource management.
- `src/css.rs`: CSS tokenization and rule parsing.
- `src/style.rs`: Style application and inheritance logic.
- `src/layout.rs`: Box model and layout engine.
- `src/render.rs`: Skia-based drawing logic.
- `src/js.rs`: Boa engine wrapper and browser API mocking.

## Roadmap
- [ ] Advanced Layout (Grid support, etc.)
- [ ] Expanded JavaScript DOM API (Event listeners, element manipulation, etc.)
- [ ] Enhanced Form Input and Interactivity
- [ ] Image Caching Optimizations and Incremental Rendering
