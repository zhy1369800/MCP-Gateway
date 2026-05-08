---
name: multi_edit_file
description: >-
  Apply multiple exact string replacements to one existing file in a single
  write. Use this for larger or repeated source edits where Codex-style
  apply_patch hunks would be noisy. First read the complete
  builtin://multi_edit_file/SKILL.md to get skillToken; this SKILL.md read does
  not require skillToken. Do not use regex or partial reads to fetch only the
  token. Calls without the correct skillToken will fail and must be retried.
metadata:
  bundled: true
  tool: multi_edit_file
  category: editing
---

# Multi Edit File

Use this bundled skill for manual edits to an existing text file when several exact replacements should be applied together. The gateway validates every edit against the current in-memory file content first, then writes the file once. If any edit fails, no file is written.

Prefer `apply_patch` for creating, deleting, moving, or renaming files. Prefer `multi_edit_file` for multiple modifications inside one existing file.

## Input

```json
{
  "path": "src/example.rs",
  "edits": [
    {
      "old_string": "old text",
      "new_string": "new text",
      "replace_all": false,
      "startLine": 42
    }
  ],
  "cwd": "D:/path/to/workspace",
  "skillToken": "..."
}
```

- `path` is the existing file to edit, relative to `cwd` unless absolute.
- `edits` is an ordered list of replacements applied to the current file content in memory during this call.
- `old_string` must be exact current file text after normalizing CRLF to LF.
- `new_string` is the replacement text.
- `replace_all` replaces every occurrence of `old_string` in the current in-memory state.
- `startLine` is optional and 1-based. Use it when the same `old_string` appears more than once and you want the closest match to that line.

## Rules

1. Read the relevant file content before editing.
2. Treat every successful write as immediately committed to disk. If this file was just changed by `multi_edit_file`, `apply_patch`, a formatter, or another tool call, base the next edit on the latest file text, not on an older snapshot.
3. Use exact `old_string` text copied from the current file, including indentation.
4. Keep `old_string` as small as practical while still unique. If it is not unique, either set `replace_all` or provide `startLine`.
5. Do not set `old_string` equal to `new_string`.
6. Do not use an empty `old_string`; use `apply_patch` for insert-only changes.
7. Order edits so a later `old_string` does not target text produced by an earlier `new_string`.
8. For TS, TSX, JS, and JSX template strings, write `${...}` exactly. Do not escape the dollar sign as `\${...}` unless the target source code truly needs a literal `${...}` string.

## Examples

Multiple targeted edits:

```json
{
  "path": "src/config.rs",
  "edits": [
    {
      "old_string": "action: SkillPolicyAction::Deny,",
      "new_string": "action: SkillPolicyAction::Confirm,",
      "startLine": 120
    },
    {
      "old_string": "reason: \"blocked\".to_string(),",
      "new_string": "reason: \"requires confirmation\".to_string(),",
      "startLine": 121
    }
  ]
}
```

Rename throughout one file:

```json
{
  "path": "src/lib.rs",
  "edits": [
    {
      "old_string": "old_name",
      "new_string": "new_name",
      "replace_all": true
    }
  ]
}
```

## Result And Events

On success, the result includes a compact delta with byte counts and affected path metadata. It also includes a `warnings` array when the gateway detects likely syntax hazards such as unbalanced delimiters or accidental `\${...}` in TS/JS files. Warnings do not block the write; inspect and verify them before continuing. The admin event stream records an `editPreview` event before policy confirmation and a `finished` event after completion.
