# archive-priority

Archive completed priority sections from PRIORITY.md.

## When to use
- User says "archive priority", "/archive-priority", "clean up priority", "priority가 너무 길어"
- PRIORITY.md has many completed (✓) sections
- Need to shrink the active priority doc

## How it works
1. Read `.agents/PRIORITY.md`
2. Read `.agents/PRIORITY-ARCHIVE.md` (create if missing)
3. Identify all sections where EVERY issue is marked ✓ (or ~~strikethrough~~ ✓)
4. Move those sections from PRIORITY.md to PRIORITY-ARCHIVE.md
5. Keep active (partially or fully pending) sections in PRIORITY.md
6. Preserve the header, active sections, and dependency graph for active sections only
7. Add link from PRIORITY.md → PRIORITY-ARCHIVE.md

## Rules
- Only archive sections where ALL rows have ✓
- Keep the "Dependency graph" section at the bottom for active sections only
- Keep "Domain closure" table for active domains only
- Archive file maintains same section order as original
