# Aura Browser 2.0: Living Engine Upgrade Plan

The goal of this phase is to transition Aura Browser from a static HTML/CSS viewer into a truly interactive, modern web browser engine where JavaScript can manipulate the page in real-time and the layout reflects modern standards.

## 1. True DOM-JS Integration (Reactive Engine)
- **Problem:** Current JS environment uses "Mock" objects that do not affect the actual Rust data structures.
- **Solution:** Implement a bidirectional bridge using Boa's `NativeObject`.
    - **Node Mapping:** Assign unique IDs to Rust DOM nodes and map them to JS `Element` objects.
    - **Manipulation API:** Implement `document.getElementById()`, `element.innerHTML`, and `element.style` so JS changes directly update the Rust Style/DOM Tree.
    - **Incremental Re-rendering:** When a JS change occurs, trigger a "Dirty" flag to re-calculate layout and re-render only the necessary parts.

## 2. Modern Layout: Flexbox Fundamentals
- **Goal:** Support `display: flex` to align with modern web design patterns.
- **Features:**
    - `flex-direction`: row, column.
    - `justify-content`: center, space-between, space-around.
    - `align-items`: center, flex-start, flex-end.
- **Implementation:** Enhance the recursive `build_layout_tree` to handle flex containers and distribute children based on flex properties.

## 3. Visual Polish & High Fidelity Rendering
- **Rounded Corners:** Implement `border-radius` parsing and rendering using `tiny-skia`'s path clipping.
- **Shadows:** Support `box-shadow` with Gaussian blur for a "depth" effect.
- **Transitions:** Basic support for CSS property transitions to make the browser feel "alive."

## 4. Enhanced Interaction & Networking
- **Event Delegation:** A centralized event loop that captures native `winit` events and dispatches them to both the GUI (Chrome) and the JS Engine (Content).
- **Fetch API:** Implement a real `window.fetch` using `reqwest` to allow pages to load data dynamically after the initial render.

## 5. UI/UX Refinement (Browser Chrome)
- **Modern UI:** Redesign the address bar and buttons using `egui`'s advanced styling (rounded corners, subtle shadows).
- **Loading Progress:** Add a visual progress bar or spinner that accurately reflects networking and parsing status.

---

## Execution Milestones
1. **Milestone A:** Real-time DOM manipulation (e.g., change background color via JS console).
2. **Milestone B:** Flexbox layout support for `yunseong.dev` menu items.
3. **Milestone C:** Visual fidelity upgrades (Rounded corners and shadows).
