# Codex Agent Instructions

## Global Codex Preferences

### Language
- English only for responses, code comments, and reasoning.
- The user is learning English. Correct grammar errors inline, naturally.

### Agent Triggers
- `"develop [X]"` should use the `developer` workflow.
- `plan-reviewer` and `code-reviewer` should only be used inside the `developer` workflow, not as standalone defaults.

### Session Context
- Read `./.agents/handoffs/LATEST.md` silently at session start if it exists and is relevant.
- Use it for ongoing context.
- Do not announce that handoff context unless it is directly relevant.

## Browser Project Guide

Browser is a Rust browser engine + native UI app.

Pipeline: `Network -> DOM -> Style -> Layout -> Render -> GUI`

### Common Commands
- `cargo build`
- `cargo build --bins`
- `cargo run`
- `cargo run --release`
- `cargo check`
- `cargo clippy`
- `cargo test --lib`
- `cargo test --test test_pipeline`
- `timeout 300s cargo test -- --test-threads=2`
- `cargo test <test_name>`

### Architecture Map
- `src/main.rs`: GUI app, fetch/image orchestration, navigation state, JS runtime integration, pipeline orchestration
- `src/dom.rs`: HTML parsing
- `src/css.rs`: CSS parser and selector specificity
- `src/style.rs`: styled tree, inheritance, inline style handling, stylesheet extraction
- `src/layout.rs`: layout tree and computed rectangles
- `src/layer_tree.rs`: paint command tree and clipping commands
- `src/render.rs`: raster painting
- `src/js.rs`: Boa-based JS runtime wrapper

### Project Rules
- Use `./.agents/PRIORITY.md` to choose the next issue.
- Follow the development workflow in `./.agents/docs/development-workflow.md`.
- Render width is fixed to `800px` unless changed in code.
- Build/check/lint on host. For daemon/CLI execution, use `drun rust:latest` (resource-limited).
- Never run long render/integration loops without timeout.
- Run `browser-cli-reviewer` before PR creation for browser implementation changes.
- Before finishing implementation work, run build + tests relevant to the touched scope.

## Priority Workflows

When the user asks for any of the following:
- `update-priority`
- `/update-priority`
- `sync priority`
- `mark done issues`

Use the prompt at `./.agents/prompts/update-priority.md`.

When the user asks for any of the following:
- `develop-priority`
- `/develop-priority`
- `developer-priority`
- `/developer-priority`
- `develop most priority issue`
- `work on next issue`
- `next priority`

Use the prompt at `./.agents/prompts/develop-priority.md`.

`./.agents/PRIORITY.md` is the canonical priority roadmap.
Do not use `docs/issue-priority.md` as the source of truth.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

When the user types `/graphify`, invoke the `skill` tool with `skill: "graphify"` before doing anything else.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- Dirty graphify-out/ files are expected after hooks or incremental updates; dirty graph files are not a reason to skip graphify. Only skip graphify if the task is about stale or incorrect graph output, or the user explicitly says not to use it.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
