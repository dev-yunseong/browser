# Agent Instructions & Memory

## Language Preferences
- **Internal thoughts:** English
- **Internal documents (e.g., this file):** English
- **Responses to the user:** Korean
- **Documents meant for the user to read:** Korean
- **Sub-agent instructions/messages:** English

## Current Project
- **Goal:** Build a web browser from scratch using Rust.
- **Key components to consider:** Networking, HTML Parsing, DOM, CSS Parsing, Layout Engine, Rendering, and JS Engine integration.
- **Priority:** Always refer to `./.agents/PRIORITY.md` for the canonical priority order and dependency graph.

## Engineering Standards & Mandates
- **Language:** Internal (English), User-facing (Korean).
- **Testing:** ALWAYS write automated test cases for every new feature or bug fix. Never settle for unverified changes.
- **Robustness:** Avoid `.unwrap()` in production code; handle errors gracefully.
- **Analysis discipline:** Do not weaken or remove important content just to pass review. Make analysis/specs more concrete instead.
- **Spec writing rule:** Separate each issue into (1) code-confirmed fact, (2) condition linking it to the observed symptom, and (3) validation item if the symptom link is not yet proven.

## Progress
- **Phase 1-6:** Basic CLI prototype with static PNG rendering complete.
- **Phase 7 (GUI Upgrade):** Done. Integrated `eframe` (egui + winit) to create a functional desktop browser.
- **Features implemented:**
    - Address bar for loading any URL.
    - Navigation buttons (Back, Forward, Refresh).
    - Navigation history management.
    - Interactive link clicking (Hit testing on Layout Boxes).
    - Visual feedback (cursor change) for links.
    - Viewport rendering using `tiny-skia` textures.

## Final Status
The Rust web browser is now a fully functional desktop application. Users can type URLs, navigate through pages, and click on links within the rendered content. The engine's pipeline (Networking -> DOM -> Style -> Layout -> Render) is fully integrated into the GUI's event loop.
