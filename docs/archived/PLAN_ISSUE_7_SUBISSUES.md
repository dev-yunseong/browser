# Implementation Plan: Resource Loading & Security Model (Issue #7)

## 1. Goal
Propose and create Depth 3 (System) sub-issues for Issue #7 to establish a robust resource loading and security framework for Aura Browser, covering Fetch API, CORS, SOP, and CSP.

## 2. Proposed Sub-issues

### Sub-issue 7.1: Functional Fetch API & Native Bridge
- **Objective**: Replace JS stubs with actual networking capability.
- **Tasks**:
  - Implement `fetch` native function in `src/js.rs` using `reqwest`.
  - Handle JS Promises in Rust (bridge `boa_engine` async-like patterns).
  - Inject the current origin into the JS context.

### Sub-issue 7.2: CORS Enforcement Layer
- **Objective**: Prevent unauthorized cross-origin data access.
- **Tasks**:
  - Implement a `CORSChecker` module.
  - Handle preflight `OPTIONS` requests.
  - Validate `Access-Control-Allow-Origin`.

### Sub-issue 7.3: Origin-Based Storage Partitioning (SOP)
- **Objective**: Isolate data between different sites.
- **Tasks**:
  - Modify `localStorage` implementation in `js_bootstrap.js` to use origin-keyed storage in Rust.
  - Implement origin validation for DOM access.

### Sub-issue 7.4: Content Security Policy (CSP) Engine
- **Objective**: Mitigate XSS and unauthorized resource loading.
- **Tasks**:
  - Create a CSP parser for the `Content-Security-Policy` header.
  - Integrate CSP checks into `fetch_and_process` in `src/main.rs`.
  - Block unauthorized `eval()` and inline scripts in `src/js.rs`.

## 3. GitHub Issue Creation Strategy
- Use `gh issue create` for each sub-issue.
- Title format: `[System] <Title> (Part of #7)`
- Assign labels: `security`, `networking`, `system`.
- Body should include "Part of #7" and detailed requirements.

## 4. Verification Plan
- Verify issue creation via `gh issue list`.
- Perform a self-review of the proposed logic against web standards.
