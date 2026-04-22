# update-priority

Purpose: sync `./.agents/PRIORITY.md` with GitHub issue state.

Workflow:

1. Run `gh issue list --state closed --limit 200 --json number,title` to collect closed issues.
2. Read `./.agents/PRIORITY.md`.
3. For each table row matching `| #N |`, if issue `N` is closed and not already marked done, wrap the title as `~~Title~~ ✓`.
4. Save `./.agents/PRIORITY.md`.
5. Report:
   - how many issues were newly marked done
   - which issue numbers changed
   - what the current top open, ready issue is

Rules:

- `./.agents/PRIORITY.md` is the only canonical priority file.
- Do not edit `docs/issue-priority.md` except to keep it as a pointer if needed.
- Preserve ordering, dependency notes, and non-status text.
