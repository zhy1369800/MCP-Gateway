---
name: task-planning
description: Use this instruction-only skill for complex multi-step work that benefits from a short todo list, progress marking, and cleanup after completion. It stores no state and runs no scripts.
metadata:
  short-description: Lightweight todo planning workflow
---

# Task Planning

Use this skill when a task has multiple dependent steps, unclear sequencing, or enough risk that a visible checklist will reduce mistakes. Do not use it for simple answers, small edits, or one-command checks.

## Workflow

1. Before starting substantial work, write a concise todo list in the conversation.
2. Keep steps outcome-oriented and small enough to verify, but do not over-split routine work.
3. Mark exactly one active step as `in_progress` when work is underway.
4. After each meaningful step, rewrite the list with current statuses.
5. If the approach changes, replace stale items with the new plan instead of preserving obsolete steps.
6. When the task is complete, stop showing the temporary todo list. The list is not stored anywhere; "deleting" it means omitting it from later responses or rewriting the final answer without it.

## Status Values

Use these status labels when showing a checklist:

- `pending` - not started yet
- `in_progress` - currently being worked on
- `completed` - finished and no longer active

## Suggested Format

```text
Plan:
- completed: Inspect existing skill registration
- in_progress: Add the task-planning bundled skill
- pending: Run focused tests
```

For final responses, summarize what changed and any verification performed. Do not keep the planning checklist in the final answer unless the user explicitly asks for it.
