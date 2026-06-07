# Development Workflow

Use this workflow for non-trivial issue work.

## Required Sequence

1. Create or select a GitHub issue.
   - Prefer an existing issue when it already covers the work.
   - Create a new issue when the blocker is narrower than the existing roadmap item.
   - Capture observed behavior, expected behavior, scope, and validation commands.

2. Create the branch.
   - Format:

```text
<tag>/<issue-number>
```

   - Examples:

```text
fix/279
feat/257
docs/279
refactor/270
test/278
```

   - `<tag>` should match the main work type: `feat`, `fix`, `docs`, `test`, `refactor`, `chore`, `build`, or `ci`.
   - `<issue-number>` must be the GitHub issue number without `#`.
   - Keep branch names short. Put details in the issue, plan, commit message, and PR body.
   - One issue should usually map to one branch.

3. Write the plan.
   - Use the project-local `writing-plan` skill.
   - Store issue-linked plans under `.plan/issues/`.
   - The plan must include goal, non-goals, context, checklist, validation, risks, rollback, and open questions.

4. Run plan review.
   - Use the project-local `multi-model-plan-review` skill before implementation.
   - Apply must-fix feedback to the plan before coding.
   - Record rejected feedback with a short reason when useful.

5. Develop.
   - Keep edits scoped to the issue and plan.
   - Prefer existing project patterns over new abstractions.
   - Run targeted tests as the implementation progresses.
   - Keep long browser/daemon/site checks wrapped in `timeout`.

6. Run pair review.
   - Use the project-local `pair-review` skill before final validation.
   - Focus review on YAGNI, DRY, maintainability, scope control, correctness, and validation gaps.
   - Fix must-fix findings before PR.

7. Validate and open PR.
   - Run build/tests relevant to touched scope.
   - Run `browser-cli-reviewer` before PR creation for browser implementation changes.
   - Push the branch and open a PR linked to the issue.
   - PR body should include issue link, summary, validation, and remaining risks.

## Local Skills

Project-local copies of the workflow skills live in `./.agents/skills/`:

- `writing-plan`
- `multi-model-plan-review`
- `pair-review`

Prefer these project-local copies for this repository so workflow behavior stays stable even if user-level skills change.
