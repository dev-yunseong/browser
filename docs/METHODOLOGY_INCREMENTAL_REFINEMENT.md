# Skill: Recursive Incremental Issue Refinement

This document defines the mandatory methodology for creating complex project roadmaps and issues within this project.

## Objective
To decompose abstract high-level goals into concrete, actionable tasks through a 7-depth hierarchical structure, ensuring quality and alignment at every level through mandatory peer review.

## The 7-Depth Hierarchy
1.  **Vision (Depth 1):** The ultimate strategic objective.
2.  **Domain (Depth 2):** Major functional areas (e.g., Layout, Networking, JS).
3.  **System (Depth 3):** Architectural components within a domain.
4.  **Module (Depth 4):** Specific logic modules or data structures.
5.  **Operation (Depth 5):** Key algorithms or functional logic.
6.  **Task (Depth 6):** Actionable coding tasks or PR-level objectives.
7.  **Detail (Depth 7):** Precise implementation details, test cases, and micro-optimizations.

## The Workflow Loop
**NEVER generate the full tree at once. Follow this recursive process:**

1.  **Define Issue:** Propose a single issue at Depth N.
2.  **Review Pass:** Invoke the `Reviewer Agent` (Generalist) to critique the issue.
3.  **Iteration:** If the Reviewer provides feedback, revise the issue until a **'Pass'** is granted.
4.  **Sub-issue Expansion:** ONLY after a 'Pass' at Depth N, proceed to define the sub-issues for Depth N+1.
5.  **Recurse:** Repeat until Depth 7 is reached for every branch.

## Role of Sub-Agents
- **Researcher:** Use `generalist` to fetch specifications (W3C, WHATWG) and summarize them to inform issue definitions.
- **Reviewer:** Use `generalist` to verify logic, compliance, and architectural integrity.

---
*Follow this methodology to ensure every line of code serves a well-defined and peer-reviewed purpose.*
