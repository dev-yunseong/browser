---
name: update-priority
description: >
  Syncs ./.agents/PRIORITY.md with GitHub: fetches all closed issues via `gh`,
  then marks each closed issue as done (✓) in the priority doc and saves it.
  Invoke when the user says "update priority", "/update-priority", "sync priority",
  "mark done issues", or wants the priority doc refreshed from GitHub.
---

1. `gh issue list --state closed --limit 200 --json number,title` — collect closed issue numbers.
2. Read `./.agents/PRIORITY.md`.
3. For each table row `| #N |`: if N is closed and not already `✓`, wrap title as `~~Title~~ ✓`.
4. Save `./.agents/PRIORITY.md`.
5. Report: how many newly marked, which numbers, and what is now the top open, ready issue.

## Rules

- `./.agents/PRIORITY.md` is the only canonical priority file.
- Do not edit `docs/issue-priority.md` except to keep it as a pointer if needed.
- Preserve ordering, dependency notes, and non-status text.
