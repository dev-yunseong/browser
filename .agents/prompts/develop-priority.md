# develop-priority

Purpose: pick the highest-priority open issue from `./.agents/PRIORITY.md` and develop it end-to-end.

Workflow:

1. Read `./.agents/PRIORITY.md`.
2. Find the first open issue top-to-bottom that is ready:
   - not already marked `✓`
   - all dependencies already done
3. Read `./.agents/handoffs/LATEST.md` if it exists.
4. Run `gh issue view <N>` for the selected issue.
5. If the issue has 4 or more independent concerns, split it:
   - create sub-issues with `gh issue create --title "..." --body "Part of #<N>"`
   - add the new issues into `./.agents/PRIORITY.md`
   - report the new issue numbers and stop
6. Otherwise execute the `developer` workflow for that issue.

Developer workflow requirements:

- For complex, multi-file, architectural work:
  - do plan review first
  - implement
  - do code review
  - run the browser CLI reviewer before PR creation
- For simple work:
  - implement directly
  - still run the browser CLI reviewer before PR creation

Rules:

- `./.agents/PRIORITY.md` is the canonical source of truth.
- Always include enough issue context when invoking sub-agents.
- Always run `browser-cli-reviewer` before creating a PR because tests alone do not verify actual rendering behavior.
