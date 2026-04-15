# Implementation Plan - Issue #43: IFC Line-Box Engine Refinement

## Goal
Refine the Inline Formatting Context (IFC) to support robust line stacking, vertical alignment (baseline, top, middle, bottom), and correct whitespace handling.

## Proposed Changes

### 1. `src/layout.rs`: Refine `Line` and Stacking Logic
- Update `Line` struct to include `ascent` and `descent`.
  ```rust
  struct Line<'a> {
      members: Vec<LayoutBox<'a>>,
      width: f32,
      height: f32,
      ascent: f32,
      descent: f32,
  }
  ```
- Implement a `get_baseline()` method for `LayoutBox`.
  - For text: Use font metrics (ascent).
  - For inline-block: Use the bottom margin edge (or the baseline of the last line box).
  - For others: Default to height.
- During line construction, track the maximum ascent and descent to determine the line's baseline and total height.

### 2. `src/layout.rs`: Implement Vertical Alignment
- Support `vertical-align` property: `baseline` (default), `top`, `middle`, `bottom`, `sub`, `super`.
- In the "Position Pass", adjust the `y` offset of each inline member based on its `vertical-align` value relative to the line's baseline or height.
  - `baseline`: Align member's baseline with line's baseline.
  - `top`: Align member's top with line's top.
  - `bottom`: Align member's bottom with line's bottom.
  - `middle`: Align member's midpoint with line's baseline + half of x-height (approx).

### 3. `src/layout.rs`: Correct Whitespace Handling
- Refactor `layout_text` to respect CSS `white-space` (initially supporting `normal` and `pre` patterns).
- Instead of `trim()`, use a logic that collapses consecutive spaces but preserves spaces between words.
- Ensure whitespace between separate inline elements (e.g., `<span>A</span> <span>B</span>`) is preserved as a single space if appropriate.

### 4. Verification Plan
- Add test cases in `tests/repro_failures.rs` (or a new test file) to verify:
  - Vertical alignment of different sized text and inline-blocks.
  - Whitespace preservation/collapsing between elements.
  - Correct line breaking when multiple inline elements are present.
- Run `cargo test`.
- Visual verification (if possible) by rendering a sample HTML.

## Architectural Considerations
- Keep the `perform_layout` function clean by potentially extracting IFC logic into a helper method if it grows too large.
- Ensure `offset_layout_box` correctly propagates offsets to all children.
