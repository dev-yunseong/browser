# Plan: High-Fidelity Rendering & Composition (Sub-issues for #5)

## Overview
This plan outlines the Depth 3 (System) sub-issues for Issue #5 ([Domain] High-Fidelity Rendering & Composition). The goal is to evolve the current immediate-mode recursive rendering into a modern, spec-compliant rendering pipeline.

## Proposed Sub-issues

### 1. [System] Stacking Context & Z-Index Management
*   **Context:** Currently, `render.rs` uses a simple two-pass (background then foreground) recursive traversal. It does not support `z-index` or stacking contexts.
*   **Objective:** Implement the CSS painting order algorithm.
*   **Key Tasks:**
    *   Update `LayoutBox` to identify stacking context triggers (e.g., `z-index` on positioned elements, `opacity < 1`, `transform`).
    *   Implement a sorting mechanism for children within a stacking context (Negative Z -> Block Flow -> Inline Flow -> Positive Z).
    *   Handle the 7-layer painting order within each stacking context as per CSS spec.

### 2. [System] Retained-Mode Display List Representation
*   **Context:** The browser paints directly to a `Pixmap` during tree traversal, which is inflexible and hard to optimize.
*   **Objective:** Introduce an intermediate `DisplayList` of paint commands.
*   **Key Tasks:**
    *   Define `PaintCommand` enum: `Rect(Rect, Paint)`, `Text(String, Point, Font)`, `Clip(Rect)`, etc.
    *   Implement a builder that traverses the `LayoutBox` tree and generates a `DisplayList`.
    *   Decouple `src/render.rs` from the layout tree, making it a "Display List Executor".

### 3. [System] Layer-Based Compositing Architecture
*   **Context:** Everything is currently drawn to a single surface. This makes animations and fixed elements expensive.
*   **Objective:** Separate the rendering into independent layers that can be composited.
*   **Key Tasks:**
    *   Identify "Composited Layers" (e.g., for `will-change`, `transform`, or `position: fixed`).
    *   Render these layers into independent textures/Pixmaps.
    *   Implement a `Compositor` that blends these layers using `tiny-skia` with support for layer-level opacity.

### 4. [System] Paint Optimization & Damage Tracking (Dirty Rects)
*   **Context:** The entire screen is repainted on every change, which is inefficient.
*   **Objective:** Implement incremental painting using dirty rectangles.
*   **Key Tasks:**
    *   Add a "Damage" tracking system to the `LayoutBox` tree.
    *   When a node changes, mark its bounding box as "dirty".
    *   Implement a rendering pass that only executes `PaintCommands` that intersect with the dirty rectangles.

### 5. [System] Advanced Visual Effects: Filters & Clipping
*   **Context:** Basic clipping exists but is manual and error-prone in `render_text_wrapped`.
*   **Objective:** Support complex CSS effects like `filter`, `mask`, and robust `overflow`.
*   **Key Tasks:**
    *   Implement a proper `ClipStack` in the `DisplayList`.
    *   Add support for Skia-based filters (e.g., `blur()`, `drop-shadow()`).
    *   Ensure `border-radius` correctly clips content (Overflow Clipping).

## Self-Review
The proposed issues cover the required areas:
*   **Stacking Contexts:** Covered by Issue 1.
*   **Display Lists:** Covered by Issue 2.
*   **Layer Compositing:** Covered by Issue 3.
*   **Paint Optimization:** Covered by Issue 4 and 5.

## Next Steps
1.  Submit this plan to the Reviewer Agent (`generalist`).
2.  Upon approval, create the issues on GitHub via `gh issue create`.
