# Plan: Implement Pseudo-class Matching (:hover)

## Goal
Implement `:hover` pseudo-class matching in the Aura Browser. When an element with an ID is hovered, styles matching `#id:hover` should be applied.

## Proposed Changes

### 1. `src/css.rs`
- **Modify `Selector` struct**:
  - Add `pub pseudo_class: Option<String>`.
- **Update `parse_selector`**:
  - Split each part of the selector by `:` using `splitn(2, ':')`.
  - Store the second part (if any) in `pseudo_class`.
  - Use the first part for existing tag, ID, and class parsing.

### 2. `src/style.rs`
- **Update `matches_selector`**:
  - Add `hovered_id: Option<&str>` to the parameters.
  - Implement logic for `:hover`:
    ```rust
    if let Some(ref pseudo) = selector.pseudo_class {
        if pseudo == "hover" {
            let mut id = None;
            if let NodeData::Element { ref attrs, .. } = handle.data {
                for attr in attrs.borrow().iter() {
                    if attr.name.local.to_string() == "id" {
                        id = Some(attr.value.to_string());
                    }
                }
            }
            if Some(id.as_deref()) != Some(hovered_id) || id.is_none() {
                return false;
            }
        } else {
            return false; // Unsupported pseudo-class
        }
    }
    ```
  - Pass `hovered_id` to recursive calls of `matches_selector`.
- **Update `build_style_tree`**:
  - Add `hovered_id: Option<&str>` to the parameters.
  - Pass `hovered_id` to `matches_selector` and recursive `build_style_tree` calls.

### 3. `src/layout.rs`
- **Add `collect_element_ids` to `LayoutBox`**:
  - Traverse the tree and collect `(Rect, String)` for all elements that have an `id`.

### 4. `src/main.rs`
- **Update `BrowserApp` struct**:
  - Add `hovered_id: Option<String>`.
  - Add `current_element_ids: Vec<(layout::Rect, String)>` to store ID-to-Rect mappings.
- **Update `StaticPageData` struct**:
  - Add `element_ids: Vec<(layout::Rect, String)>`.
- **Update `process_html_with_cache` and `fetch_and_process`**:
  - Add `hovered_id: Option<&str>` parameter.
  - Use `layout_tree.collect_element_ids(&mut element_ids)` to populate the new field in `StaticPageData`.
- **Update `BrowserApp::update`**:
  - Detect hovering in the content area:
    ```rust
    let mut new_hovered_id = None;
    if let Some(ptr) = response.hover_pos() {
        let rel = ptr - rect.min;
        // Search in reverse to find the innermost (top-most in layout) element
        for (l_rect, id) in self.current_element_ids.iter().rev() {
            if hit(rel, l_rect) {
                new_hovered_id = Some(id.clone());
                break;
            }
        }
    }
    if new_hovered_id != self.hovered_id {
        self.hovered_id = new_hovered_id;
        self.trigger_re_render(ctx, ui.available_width());
    }
    ```
- **Update `trigger_re_render` and other call sites**:
  - Pass `self.hovered_id` (converted to `Option<&str>`).

## Verification Plan
1. **Automated Check**: Run `cargo check` to ensure no compilation errors.
2. **Manual Verification**: Create a test HTML with a hover effect on an ID and verify it works (or simulate via test case).
3. **Review**: Invoke 'Reviewer Agent' to compare Plan vs. Code after implementation.
