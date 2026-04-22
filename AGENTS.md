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
