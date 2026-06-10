---
name: develop-priority
description: >
  Picks the highest-priority open issue from ./.agents/PRIORITY.md and develops
  it end-to-end using the developer agent (plan → implement → review → PR).
  Invoke when the user says "develop priority", "/develop-priority",
  "developer-priority", "/developer-priority", "develop most priority issue",
  "work on next issue", or "next priority".
---

1. Read `./.agents/PRIORITY.md`. Find the first open (no ✓), ready (deps all ✓) issue top-to-bottom.
2. Read `./.agents/handoffs/LATEST.md` for prior context.
3. `gh issue view <N>` — get full issue body.
4. Before starting implementation, update the selected issue row in `./.agents/PRIORITY.md` to show it is being developed by the active agent identity:
   - preserve the table structure
   - preserve any existing done marker formatting
   - append ` (in progress by <agent_id>)` to the issue title if no in-progress marker is already present
   - use the CLI program and model name as `<agent_id>`, for example `codex:gpt-5.4`
5. **Scope check**: if issue has 4+ independent concerns → create sub-issues (`gh issue create --title "..." --body "Part of #<N>"`), add to priority doc, report numbers, stop.
6. Invoke `developer` agent with: issue number + full body, relevant `CLAUDE.md` context, and handoff decisions.

## Rules

- `./.agents/PRIORITY.md` is the canonical source of truth.
- Always include enough issue context when invoking sub-agents.
- Run project-specific validation before creating a PR; tests alone may not verify actual user-facing behavior.
