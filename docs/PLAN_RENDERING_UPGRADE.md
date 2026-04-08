# Rendering Engine Refactoring Plan: Modern Compositor Architecture

This document outlines the 7-depth strategic roadmap for transitioning the rendering engine from a direct layout-to-pixel approach to a modern, layer-based compositing system.

---

## Depth 1: Strategic Vision (The Goal)
### Issue #1: High-Performance, Correct Rendering Engine
Transform the browser's rendering pipeline to match modern standards, ensuring correct visual stacking, high-performance updates, and modularity.

---

## Depth 2: Tactical Architecture (The Strategy)
### Issue #2: Layer-Based Compositing & Stacking Contexts
Introduce the concept of "Layers" to handle complex CSS stacking rules (Z-index, Opacity, Transforms) correctly.
- **Sub-Issue #2.1:** Implement Stacking Context resolution logic in `src/style.rs`.
- **Sub-Issue #2.2:** Define a `LayerTree` structure that represents independent paint layers.

---

## Depth 3: Structural System (The Framework)
### Issue #3: Decoupling Painting from Rasterization (Display Lists)
Move away from "drawing pixels while traversing the tree" to "generating commands while traversing, then executing commands".
- **Sub-Issue #3.1:** Define `PaintCommand` enum (DrawRect, DrawText, etc.).
- **Sub-Issue #3.2:** Implement `DisplayList` container to hold sequences of commands for each layer.

---

## Depth 4: Functional Module (The Manager)
### Issue #4: Layer Management System
Logic to manage layer lifecycle, dirty regions, and final composition.
- **Sub-Issue #4.1:** Implement `Compositor` to merge multiple layers into the final `Pixmap`.
- **Sub-Issue #4.2:** Add clipping support at the layer level.

---

## Depth 5: Operational Primitives (The Tools)
### Issue #5: Specialized & Optimized Paint Primitives
Optimize how individual shapes and text are rendered within the command buffer.
- **Sub-Issue #5.1:** Refactor `render_text_wrapped` into a high-performance `TextPaint` primitive.
- **Sub-Issue #5.2:** Implement dedicated `ImagePaint` with caching and scaling support.
- **Sub-Issue #5.3:** Support for advanced effects like `opacity` and `border-radius`.

---

## Depth 6: Implementation Logic (The Code)
### Issue #6: Refactor `src/render.rs` and `src/main.rs`
Rewrite the rendering entry points to utilize the new compositor and display list system.
- **Sub-Issue #6.1:** Transition `render_layout_tree` to `generate_display_list`.
- **Sub-Issue #6.2:** Update `BrowserApp::update` to trigger compositor passes.

---

## Depth 7: Surgical Refinement (The Precision)
### Issue #7: Micro-Optimizations & Visual Fidelity
Fine-tune the individual rendering operations for maximum visual quality.
- **Sub-Issue #7.1:** Precise pixel blending algorithm refinement.
- **Sub-Issue #7.2:** Anti-aliasing improvements for fonts and borders.
- **Sub-Issue #7.3:** Bounds checking and memory safety audits in pixel buffers.

---

## Verification Plan (Verify)
Each depth will be verified by the `Reviewer Agent` comparing the implementation against these requirements.
1. **Pass criteria:** Successful rendering of complex Z-index sites.
2. **Performance criteria:** Measurable reduction in re-paint time for large pages.
