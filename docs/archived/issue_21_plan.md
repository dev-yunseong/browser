# Plan - Issue #21: Full CSS Syntax Parser

Implement a robust CSS syntax parser using the `cssparser` crate to replace the current fragile manual string manipulation.

## 1. Research & Analysis
- Current `src/css.rs` uses `split`, `replace`, and manual loops, which fail on nested structures and complex selectors.
- `cssparser` is already in `Cargo.toml`.
- `src/style.rs` depends on `Stylesheet` and `Rule` structures.

## 2. Proposed Changes

### Data Structures (`src/css.rs`)
- Introduce `AtRule` enum:
  ```rust
  pub enum AtRule {
      Media { query: String, rules: Vec<Rule> },
      Import(String),
      // ... other at-rules
  }
  ```
- Update `RuleOrAtRule` enum:
  ```rust
  pub enum RuleOrAtRule {
      Rule(Rule),
      AtRule(AtRule),
  }
  ```
- Update `Stylesheet`:
  ```rust
  pub struct Stylesheet {
      pub rules: Vec<RuleOrAtRule>,
  }
  ```

### Parsing Logic (`src/css.rs`)
- Implement `cssparser::DeclarationParser` for properties.
- Implement `cssparser::AtRuleParser` for `@media`, etc.
- Implement `cssparser::QualifiedRuleParser` for standard rules.
- Improve `Selector` parsing to handle complex cases correctly using `cssparser` tokens.
- Ensure error recovery (skip invalid rules/declarations).
- Preserve unknown properties as `Value::Keyword`.

### Style Tree Integration (`src/style.rs`)
- Update `build_style_tree` to handle `RuleOrAtRule`.
- Initially, ignore at-rules or implement basic `@media` support (optional but recommended since the goal is to "stop stripping them").

## 3. Implementation Steps

1. **Step 1: Refactor Data Structures**
   - Add `RuleOrAtRule` and `AtRule`.
   - Update `Stylesheet`.

2. **Step 2: Implement `cssparser` Boilerplate**
   - Create a struct that implements `QualifiedRuleParser`, `AtRuleParser`, and `DeclarationParser`.

3. **Step 3: Implementation of Parser**
   - Re-implement `parse_css` using `RuleListParser`.
   - Implement property value parsing within the `cssparser` framework.
   - Refactor `parse_selector` to work with `cssparser` tokens or at least be more robust.

4. **Step 4: Update `src/style.rs`**
   - Modify the rule matching loop to handle the new `Stylesheet` structure.

5. **Step 5: Verification**
   - Run existing tests.
   - Add new tests for at-rules and complex CSS.
   - `cargo build` and `cargo test`.

## 4. Testing Strategy
- Test with standard CSS.
- Test with `@media` blocks.
- Test with invalid CSS to ensure recovery.
- Test with unknown properties.
