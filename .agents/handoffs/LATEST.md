# Handoff — 2026-05-05 17:10

## Summary
Completed Priority 13 (Google Search form submission), fixed multiple form/GUI bugs, started Priority 14 (V8 JS engine migration planning). Created archive-priority skill and shrunk PRIORITY.md from 286→35 lines.

## Done
- **#225**: FormMetadata/FormControlMeta structs + collect_form_element() in engine.rs/layout.rs
- **#226**: EngineCmd::Submit + BrowserEngine::submit_form() — builds GET URL from form metadata + JS DOM values
- **#227**: Enter-key form submission in GUI (main.rs + browser_daemon.rs)
- **#228**: resolve_url() — URL bar search fallback (domain → https://, text → Google search)
- **CLI T6 fix**: click_by_index/click_by_text fallback to daemon GET /page when last_page is None
- **Form button fix**: submit/button/reset inputs excluded from TextEdit overlay, rendered as clickable egui::Button
- **Value sync fix**: form_values synced to JS DOM before submit (querySelector + value assignment)
- **<button> support**: collect_form_controls now matches "button" tag, extract_text_content() helper
- **DisplayType fix**: form controls keep DisplayType::Input even with CSS display:block (unblocked yunseong.dev)
- **Buttons excluded from submit URL**: btnG/btnI no longer appear in form_metadata.controls
- **Console closed by default**: console_panel_open: false in both GUI and daemon
- **Stack overflow prevention**: engine-actor thread stack 8MB (from default 2MB)
- **Priority cleanup**: PRIORITY-ARCHIVE.md with 120 resolved issues, PRIORITY.md shrunk to 35 lines
- **archive-priority skill**: global skill for future priority archiving (~/.agents/skills/archive-priority/)

## Current State
main branch: all Priority 0-13 issues resolved. Browser-tester 7/7 PASS.
Next: Priority 14 — V8 JS Engine Migration (#232-#237).

### V8 Issues Created
| # | Phase | Depends |
|---|---|---|
| #232 | Umbrella | - |
| #233 | Phase 1: rusty_v8 + isolate + script exec | - |
| #234 | Phase 2: DOM bindings | #233 |
| #235 | Phase 3: Event system + timers | #234 |
| #236 | Phase 4: fetch, XHR, storage, CSSOM, forms | #235 |
| #237 | Phase 5: Integration + testing | #236 |

### Key patterns for V8 migration
- src/js.rs: 4449 lines, 92 public functions/structs
- Current API: JsRuntime::new(), execute(), execute_with_result(), tick(), trigger_event()
- Boa patterns: Context, register_global_callable, JsValue, js_string → V8 patterns: Isolate, Context, HandleScope, FunctionTemplate
- Thread model: engine actor is single-threaded (reqwest blocking), V8 isolate must stay on same thread

## Decisions
| Decision | Reason |
|----------|--------|
| rusty_v8 over rquickjs | Full ES2024 + JIT, prebuilt binaries (no source build). rquickjs is lighter but less capable |
| Exclude button values from submit URL | Real browsers only send clicked button's value. Enter-key submit sends no button values |
| Form controls keep DisplayType::Input with CSS display:block | Bootstrap .form-control breaks form detection otherwise. CSS block on inputs means "fill width" not "become div" |
| Archive completed priorities | PRIORITY.md was 286 lines of mostly ✓. Active work is only P14 |

## Next Steps
1. `develop-priority` → starts #233 (V8 Phase 1)
2. Or `develop-priority` to start #232 umbrella issue
3. After V8 phases complete: test chatgpt.com, naver.com, yunseong.dev search

## Blockers / Open Questions
- None on V8 migration
