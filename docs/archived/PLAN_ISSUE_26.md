# Plan: Issue #26 - Native Fetch API & JS Promise Bridge

## 1. Objective
Implement a functional `window.fetch` in the Aura browser that supports the standard Promise-based API: `fetch(url).then(r => r.text()).then(data => ...)`.

## 2. Technical Strategy

### 2.1. Task Queue & Event Loop Integration
*   **Problem:** `boa_engine`'s `Promise` jobs need to be executed via `context.run_jobs()`. Also, background network requests need a way to "wake up" the JS runtime on the main thread.
*   **Solution:**
    *   Modify `JsRuntime::run_queued_tasks` to also call `self.context.run_jobs()`.
    *   Introduce a thread-safe channel or a shared task queue to allow background threads (network fetches) to post callbacks back to the main thread.
    *   Since `MACRO_TASKS` is currently `thread_local`, I will keep it for now but add a mechanism to inject tasks from other threads, or use a `std::sync::mpsc` channel in `JsRuntime` that is checked during `run_queued_tasks`.

### 2.2. Native Bridge (`src/js.rs`)
*   Implement `__aura_fetch(url: String)` as a `NativeFunction`.
*   Inside `__aura_fetch`:
    1.  Create a `JsPromise` using `JsPromise::new(&mut context)`.
    2.  Capture the `Promise` and its `resolver`.
    3.  Spawn a background thread (using `std::thread::spawn`).
    4.  In the background thread:
        *   Use `reqwest::blocking::get(url)` to fetch the resource.
        *   On completion, send the result (status, headers, body) back to the main thread via a channel.
    5.  Return the `Promise` to JS.

### 2.3. Main Thread Integration (`src/main.rs` & `src/js.rs`)
*   Update `JsRuntime` to hold a receiver for background tasks.
*   In `BrowserApp::update`, when `js_runtime.run_queued_tasks()` is called, it will first drain the receiver and push those tasks into `MACRO_TASKS`.
*   The task will then be executed, resolving or rejecting the JS Promise with a `Response` object.

### 2.4. JS Environment (`src/js_bootstrap.js`)
*   Define a proper `Response` class.
*   `Response` will store the results from the native fetch.
*   `Response.prototype.text()` and `Response.prototype.json()` will return Promises that resolve to the body content.
*   Actually, for simplicity in the first version, the native bridge can return the whole body, and `Response.text()` can return a pre-resolved Promise.

### 2.5. Refined Native `Response` Object
*   When the network request finishes, the native code will create a JS object representing the response.
*   This object will be passed to the Promise resolver.

## 3. Implementation Steps

1.  **Modify `src/js.rs`:**
    *   Add `use std::sync::mpsc::{channel, Sender, Receiver};`.
    *   Add `task_sender: Sender<Box<dyn FnOnce(&mut Context) + Send>>` and `task_receiver: Receiver<Box<dyn FnOnce(&mut Context) + Send>>` to `JsRuntime` (or use a global static sender if needed, but per-runtime is better).
    *   Register `__aura_fetch`.
    *   Update `run_queued_tasks` to drain the receiver and call `context.run_jobs()`.

2.  **Modify `src/js_bootstrap.js`:**
    *   Implement `window.fetch` using `__aura_fetch`.
    *   Implement `Response` class with `text()`, `json()`, `ok`, `status`, etc.

3.  **Update `src/main.rs`:**
    *   Ensure `js_runtime.run_queued_tasks()` is called frequently (already is).

4.  **Verification:**
    *   Create a test script: `fetch('https://httpbin.org/get').then(r => r.json()).then(j => console.log('Fetched: ' + JSON.stringify(j)))`.
    *   Verify output in logs.

## 4. Risks & Considerations
*   **Thread Safety:** `boa_engine`'s `Context` and `JsValue` are not `Send`. We must ensure all interactions with them happen on the main thread.
*   **Blocking:** Network requests must stay off the main thread to keep the browser responsive.
*   **Version Compatibility:** Ensure `boa_engine` 0.21.1 APIs for `JsPromise` are used correctly.
