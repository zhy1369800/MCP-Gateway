---
name: task-planning
description: Use this bundled skill to create, update, and clear the in-memory todo state required before real bundled tool calls when task planning is enabled.
metadata:
  short-description: Lightweight todo planning workflow
---

# Task Planning

Use this skill when bundled tool calls require a plan. The gateway stores a lightweight in-memory plan state per client/session and per content-derived `planningId`. It does not persist plans across gateway restarts and it does not render UI.

## Workflow

1. First read this complete SKILL.md and use the returned `skillToken`.
2. Before a real bundled tool call, call `task-planning` with `action: "update"` and the current todo list.
3. Use the returned `planningId` on the next real bundled tool call.
4. Reuse the same `planningId` across multiple tool calls while working through the plan. It does not expire after successful tool calls.
5. Do not update the plan after every tool call just because a tool ran. A single plan step may require many tool calls.
6. The gateway may include a single `planningReminder` field on successful bundled tool results. It only refers to the current `in_progress` item and may include a concise `set_status` call to mark that item completed.
7. If the same `planningId` uses `shell_command` three or more times in a row without a plan update or another successful bundled tool, the gateway may include `shellCommandReminder`. Use it as a nudge to combine related shell commands, read multiple files in one pass when useful, or prefer efficient search commands such as `rg` and `rg --files`. A shell command that starts with `rg` resets this counter.
8. If `multi_edit_file` fails three or more times in a row for the same `planningId`, the gateway may include `editFailureReminder`. Use it as a nudge to inspect exact file content, simplify the operation, split unrelated changes, or fall back to a focused shell script when structured edits keep failing.
9. If the approach or step list changes, call `action: "update"` with the new plan and use the returned new `planningId`. Stop using the old `planningId`; clear it if needed.
10. Use `action: "set_status"` for simple state changes. If `item` is omitted, it updates the current `in_progress` item. When that item is set to `completed` and pending items remain, the gateway automatically starts the next pending item as `in_progress` and returns it as `nextItem`.
11. When every plan item is updated to `completed`, the gateway closes that `planningId` and it can no longer be used for real bundled tool calls. Then omit the temporary todo list from the final response unless the user asks for it.

## Status Values

Use these status labels when showing a checklist:

- `pending` - not started yet
- `in_progress` - currently being worked on
- `completed` - finished and no longer active

## Suggested Format

```json
{
  "action": "update",
  "explanation": "Starting the implementation",
  "plan": [
    { "step": "Inspect existing skill registration", "status": "completed" },
    { "step": "Add the planning gate", "status": "in_progress" },
    { "step": "Run focused tests", "status": "pending" }
  ],
  "skillToken": "<token from this SKILL.md>"
}
```

The response includes:

```json
{
  "planning": {
    "planningId": "plan-..."
  }
}
```

Pass those fields to the next real bundled tool call:

```json
{
  "planningId": "plan-..."
}
```

For concise status updates, prefer `set_status` instead of sending the full plan again:

```json
{
  "action": "set_status",
  "planningId": "plan-...",
  "status": "completed",
  "skillToken": "<token from this SKILL.md>"
}
```

That marks the current `in_progress` item as completed. If more work remains, the response includes `nextItem`; continue with that item using the same `planningId`. To update a specific array item, use a 1-based `item` number:

```json
{
  "action": "set_status",
  "planningId": "plan-...",
  "item": 2,
  "status": "in_progress",
  "skillToken": "<token from this SKILL.md>"
}
```

For final responses, summarize what changed and any verification performed. Do not keep the planning checklist in the final answer unless the user explicitly asks for it.
