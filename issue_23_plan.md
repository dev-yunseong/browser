# Plan: Issue #23 - Live DOM Mutation API (appendChild)

## 1. Goal
Implement a live DOM mutation API for `appendChild` in the Aura Browser, allowing JavaScript to dynamically add elements to the DOM tree and have them reflected in the layout and rendering.

## 2. Proposed Changes

### A. src/js.rs
1.  **Update `JsRuntime` Struct**:
    - Add `dom: Option<markup5ever_rcdom::Handle>` field to store a reference to the current DOM tree.
2.  **Update `JsRuntime::new`**:
    - Add `dom: Option<markup5ever_rcdom::Handle>` parameter.
3.  **Implement DOM Traversal Helper**:
    - Add `find_element_by_id(handle: &markup5ever_rcdom::Handle, id: &str) -> Option<markup5ever_rcdom::Handle>`.
    - It will traverse the tree and check the `id` attribute of `Element` nodes.
    - Special case: If `id` is "body", it should also match the `<body>` tag if no explicit ID "body" is found.
4.  **Implement `__aura_append_child` Native Function**:
    - Registered as a native function in the JS context.
    - Signature: `__aura_append_child(parentId: String, childTag: String)`.
    - Implementation:
        - Finds the parent node using `find_element_by_id`.
        - If found, creates a new `markup5ever_rcdom::Node` with `NodeData::Element`.
        - Sets the tag name (`QualName`) to `childTag` in the HTML namespace.
        - Appends the new node to `parent.children`.
5.  **Imports**:
    - Add `use html5ever::{QualName, LocalName, Namespace, ns, local_name};`.
    - Add `use markup5ever_rcdom::{Node, NodeData, Handle};`.

### B. src/js_bootstrap.js
1.  **Update `document.createElement`**:
    - Ensure it stores the tag name in the created object (e.g., `el.tagName = tag.toUpperCase();`).
2.  **Update `Element.appendChild`**:
    - Change from a no-op to:
      ```javascript
      appendChild: function(child) {
          if (child && child.tagName) {
              __aura_append_child(this._id, child.tagName.toLowerCase());
          }
          return child;
      }
      ```
3.  **Update `document.body`**:
    - Initialize it as a full element using `_makeElement('body')` instead of a plain object.

### C. src/main.rs
1.  **Update `BrowserApp` Struct**:
    - Add `current_dom: Option<markup5ever_rcdom::Handle>` to persist the DOM tree.
2.  **Update Page Load Logic**:
    - In the `content_promise` Ready block:
        - Parse HTML once to get `dom_tree`.
        - Set `self.current_dom = Some(dom_tree.document.clone())`.
        - Initialize `self.js_runtime = js::JsRuntime::new(self.current_dom.clone())`.
3.  **Update `trigger_re_render` & `process_html_with_cache`**:
    - Modify `process_html_with_cache` to accept `dom_handle: Option<Handle>`.
    - If `dom_handle` is `Some`, use it instead of re-parsing the HTML body.
    - This ensures JS mutations are preserved during re-renders.

## 3. Verification Strategy
1.  **Automated Tests**:
    - Add a test case in `src/js.rs` that:
        - Initializes `JsRuntime` with a simple DOM.
        - Executes JS: `document.getElementById('root').appendChild(document.createElement('div'))`.
        - Checks if the DOM tree has the new child.
2.  **Manual Verification**:
    - Create a local HTML with a button that appends a div on click.
    - Run the browser and verify the new element appears.

## 4. Final Steps
1.  Commit changes to branch `feature/issue-23`.
2.  Push to remote.
3.  Create PR.
