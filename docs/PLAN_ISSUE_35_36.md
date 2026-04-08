# Implementation Plan - Combinator and Attribute Matching

## Goal
Update `src/style.rs` -> `matches_selector` to support CSS Combinators (`>`, `+`, `~`) and Attribute Selectors (`[attr]`, `[attr=val]`).

## Proposed Changes

### 1. Implement Attribute Matching
In `matches_selector`, after checking tag, ID, and classes, iterate through `selector.attributes`.
For each `AttributeSelector`:
- Iterate through the element's attributes (`attrs.borrow()`).
- If the name matches:
    - If `AttributeMatch::Exists`, it's a match.
    - If `AttributeMatch::Equals(val)`, check if the attribute value exactly matches `val`.
- If no matching attribute is found for a required attribute selector, return `false`.

### 2. Implement Combinator Matching
Refactor the ancestor/combinator logic in `matches_selector`.
If `selector.ancestor` exists:
- Get the `parent` of the current node.
- Based on `selector.combinator` (defaulting to `Descendant` if `None`):
    - **`Combinator::Child` (`>`)**:
        - If no parent, return `false`.
        - Return `matches_selector(ancestor_sel, &parent)`.
    - **`Combinator::Descendant` (space)**:
        - Current recursive implementation: loop through all ancestors and return `true` if any match `ancestor_sel`.
    - **`Combinator::NextSibling` (`+`)**:
        - If no parent, return `false`.
        - Find the current node's index in `parent.children`.
        - If index > 0, check `parent.children[index - 1]`.
        - Return `true` if that sibling matches `ancestor_sel`.
    - **`Combinator::SubsequentSibling` (`~`)**:
        - If no parent, return `false`.
        - Find the current node's index in `parent.children`.
        - Check all siblings with index `< current_index`.
        - Return `true` if any match `ancestor_sel`.

### 3. Utility for Parent and Sibling Access
- Use `handle.parent.take()` and `p.upgrade()` pattern to safely access the parent (already used in existing code).
- To find index in parent:
    ```rust
    let children = parent.children.borrow();
    let index = children.iter().position(|child| std::ptr::eq(child, handle));
    ```

## Verification Plan
1. **Automated Tests**:
   - Create a new test file `tests/repro_combinators_attrs.rs` (or add to `repro_failures.rs`).
   - Test cases:
     - `div[data-test]`
     - `div[data-test="val"]`
     - `div > p`
     - `h1 + p`
     - `h1 ~ p`
2. **Cargo Check**: Run `cargo check` to ensure no compilation errors.
3. **Manual Verification**: Run with a sample HTML containing these patterns.
