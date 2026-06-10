# 2026-06-08 — Fix Script and Style innerHTML Escaping

- Date: 2026-06-08
- GitHub Issue: #257
- Status: Planning

## Goal

Resolve the `JSON.parse` SyntaxError on Naver.com by ensuring that elements which contain raw text (such as `<script>` and `<style>` tags) have their contents serialized without HTML-escaping (e.g., converting `"` to `&quot;`) when retrieving their `innerHTML` or `outerHTML`.

## Non-goals

- Altering text escaping rules for standard HTML elements (e.g., `<div>`, `<span>`) where escaping is correct and required.
- Full compliance with XML serialization rules.

## Context / Constraints

- In `src/js.rs`, the serialization of DOM nodes (`serialize_node`) always calls `html_escape` on any `NodeData::Text` content.
- However, standard HTML serialization rules state that text children of specific raw-text or escapable-raw-text elements (such as `script`, `style`, `iframe`, `xmp`, `noembed`, `noframes`, `plaintext`) should be serialized as raw text rather than HTML-escaped text.
- If these are escaped, `script.innerHTML` for a script like `<script id="json">{"test": true}</script>` returns `{&quot;test&quot;: true}`, causing `JSON.parse` to crash with a `SyntaxError`.

## Approach (Checklist)

- [ ] **Step 1: Modify `serialize_node` in `src/js.rs` to support raw text serialization**
  - Change the signature of `serialize_node` to accept `parent_tag: Option<&str>`.
  - When serializing a `NodeData::Text`, if `parent_tag` is one of the raw-text tags (`script`, `style`, `iframe`, `xmp`, `noembed`, `noframes`, `plaintext`), output the text content directly without escaping.
  - When recursing into children of a `NodeData::Element`, pass the element's tag name (lowercased) as the `parent_tag`.
- [ ] **Step 2: Update all callers of `serialize_node` in `src/js.rs`**
  - Update recursive calls inside `serialize_node`.
  - Update `serialize_inner_html` to determine the tag name of the target node (if it is an Element) and pass it to `serialize_node` for its direct children.
  - Update `get_outer_html_cb` to retrieve the parent node's tag (if any) and pass it to `serialize_node`.
- [ ] **Step 3: Add unit tests in `src/js.rs`**
  - Add a unit test to verify that the `innerHTML` and `textContent` of a `<script>` element containing JSON or special characters return the unescaped raw string.
  - Verify that standard tags (like `<div>`) still have their `innerHTML` properly escaped.
- [ ] **Step 4: Verification**
  - Run `cargo test --lib` to ensure all tests pass.
  - Restart the browser daemon and verify that navigating to `https://www.naver.com` executes successfully without SyntaxErrors, and capture a verification screenshot.

## Validation

- **Commands to run:**
  - `cargo check`
  - `cargo test --lib`
- **Expected output:**
  - Build compiles, and tests pass.
  - Naver screenshot renders successfully with dynamic React elements mounted.

## Risks & Rollback

- **Risks:** Minimal, as this conforms to the standard browser HTML serialization specifications and specifically target tags that only contain raw text.
- **Rollback steps:** Revert modifications in `src/js.rs`.

## Open Questions

None.
