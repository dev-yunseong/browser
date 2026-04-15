# Plan: Issue #12 - Standard Flexbox Layout Algorithm

## Goal
Implement a more complete and standard-compliant Flexbox layout algorithm in `src/layout.rs`, supporting `flex-direction`, `flex-wrap`, and `flex-grow/shrink`.

## Proposed Changes

### 1. `src/css.rs`
- Add `Number(f32)` to `Value` enum to handle `flex-grow` and `flex-shrink`.
- Update `parse_value` to detect and parse plain numeric values.

### 2. `src/layout.rs`
- Add `FlexContainer` and `FlexItem` structures or similar to manage flex-specific state.
- Implement `layout_flex` on `LayoutBox`.
- Update `perform_layout` to dispatch to `layout_flex` when `display: flex`.

#### Flexbox Algorithm Steps:
1. **Axis Identification**: Determine main and cross axis based on `flex-direction`.
2. **Item Collection**: Filter children that are part of the flex layout.
3. **Main Size Determination**: Calculate the "flex base size" for each item.
4. **Line Breaking**: If `flex-wrap: wrap`, group items into multiple lines.
5. **Main Axis Spacing**:
    - Calculate total grow/shrink factors.
    - Distribute remaining space in each line.
6. **Cross Axis Spacing**:
    - Determine line height (max height of items in the line, or container height if single line and stretched).
    - Position items according to `align-items` / `align-self`.
7. **Justification**: Position items according to `justify-content`.

### 3. Testing
- Add test cases in `tests/repro_failures.rs` or a new test file to verify:
    - `flex-direction: column`
    - `flex-wrap: wrap`
    - `flex-grow` distribution.
    - `justify-content` and `align-items`.

## Verification Strategy
- **Pre-implementation Review**: Ask Reviewer Agent to check the plan.
- **Post-implementation Verification**: Compare implemented code with the plan.
- **Build & Test**: Run `cargo build` and `cargo test`.
