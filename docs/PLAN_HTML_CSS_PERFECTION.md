# HTML & CSS Engine Perfection Plan

## Motivation
Recent profiling on complex, real-world web pages (like GitHub or Bootstrap-heavy sites) revealed severe performance bottlenecks and memory exhaustion (OOM kills). 
Specific findings:
- **Blocking CSS Fetches**: `collect_css_in_order` performs synchronous HTTP requests during DOM processing, taking >1.3s just to download CSS.
- **OOM / Crash**: The process is killed on massive DOMs, indicating either a stack overflow during recursive layout/styling or excessive memory allocation (e.g., creating thousands of `HashMap`s for every node's style).
- **Selector Matching**: CSS selector matching is currently unoptimized and evaluates every rule against every node.

## Goal
Transform the HTML and CSS engines from a functional prototype into a robust, high-performance, and memory-efficient system capable of rendering modern web pages without crashing.

## Phase 1: Network & Resource Pipeline
**Issue: Parallel Async CSS Fetching & Processing**
- Move `reqwest::blocking::get` out of the synchronous DOM traversal.
- Implement parallel fetching for `<link rel="stylesheet">`.
- Preload scanner: parse HTML tokens and fetch CSS/JS immediately before full DOM construction.

## Phase 2: Memory Optimization (OOM Prevention)
**Issue: Style Tree Memory Optimization**
- Currently, `PropertyMap` allocates a new `HashMap<String, String>` for every single styled node. On a page with 10,000 nodes, this causes massive memory fragmentation and usage.
- **Solution**: Implement string interning for CSS keys/values, or use `Arc<HashMap>` / Copy-on-Write (Cow) to share unchanged styles from parents/defaults.

## Phase 3: CSS Parser & Selector Engine
**Issue: CSS Parser & Selector Matching Optimization**
- **Right-to-Left Matching**: CSS selectors should be evaluated from right to left (standard browser behavior).
- **Selector Indexing**: Group CSS rules by their key selector (ID, Class, Tag) in a hash map to avoid O(N*M) matching complexity.
- **Caching**: Cache matched styles for identical DOM signatures.

## Phase 4: Layout Scalability
**Issue: Layout Engine Recursion & Scalability**
- Deeply nested DOM trees cause stack overflows during recursive layout (`traverse`, `build_layout_tree`).
- **Solution**: Convert recursive tree traversals into iterative algorithms using explicit stacks allocated on the heap, or implement incremental layout constraints.
