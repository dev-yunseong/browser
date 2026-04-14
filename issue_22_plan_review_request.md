### Plan for Review: Issue #22: Event Loop: Macro/Micro Task Integration

#### Objective
Unify the task queues in `JsRuntime` and ensure spec-compliant interleaving of macrotasks, microtasks, and rendering steps.

#### Proposed Changes
1.  **Unified `tick` method in `JsRuntime`**:
    *   Implement `tick(timestamp, deadline_ms)` in `JsRuntime`.
    *   Order of operations:
        1. Drain `task_receiver` into `MACRO_TASKS`.
        2. Execute ONE task from `MACRO_TASKS` if available.
        3. Perform microtask checkpoint (process all `MICRO_TASKS` + `context.run_jobs()`).
        4. If `timestamp` is provided (rendering opportunity):
            a. Execute ALL `RAF_TASKS`.
            b. Perform microtask checkpoint.
        5. If `deadline_ms` is provided and time remains:
            a. Execute `IDLE_TASKS` until deadline or empty.
            b. Perform microtask checkpoint after each idle task.
2.  **Refine Task Registration**:
    *   Update `trigger_event` to queue a macro task instead of executing immediately.
    *   Ensure `setTimeout` and `fetch` results correctly use the macro task queue.
3.  **Integration with `main.rs`**:
    *   Replace individual `poll_` calls with a single `js_runtime.tick` call.
    *   Use `tick` return value to decide if `trigger_re_render` is needed.

#### Verification Strategy
- New test `tests/test_event_loop_complex.rs` with interleaved Promise and setTimeout.
- Verify `requestAnimationFrame` execution order relative to rendering.

### Question for Reviewer
Does this unified `tick` approach correctly capture the priority and interleaving required by the HTML spec? Are there any risks with draining ALL `RAF_TASKS` in one tick vs. the spec?
