---
name: multi_edit_file
description: >-
  Apply validated text file operations through the gateway: exact replacements,
  multi-file edits, file creation, deletion, and moves. First read the complete
  builtin://multi_edit_file/SKILL.md to get skillToken; this SKILL.md read does
  not require skillToken. Do not use regex or partial reads to fetch only the
  token. Calls without the correct skillToken will fail and must be retried.
metadata:
  bundled: true
  tool: multi_edit_file
  category: editing
---

# Multi Edit File

Use this bundled skill for manual text-file changes. The gateway resolves all affected paths, validates the full request, applies exact replacements in memory, and only then commits the file operations. If validation fails, no file is written.

Prefer `multi_edit_file` over shell redirection or ad hoc scripts for normal source/config/docs edits because the gateway can audit affected paths, enforce allowed-directory policy, and return a compact delta.

## Input Modes

Legacy single-file edit:

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

Multi-file exact edits:

```json
{
  "files": [
    {
      "path": "src/a.rs",
      "edits": [
        {
          "old_string": "old",
          "new_string": "new"
        }
      ]
    },
    {
      "path": "src/b.rs",
      "edits": [
        {
          "old_string": "enabled: false",
          "new_string": "enabled: true"
        }
      ]
    }
  ],
  "cwd": "D:/path/to/workspace",
  "skillToken": "..."
}
```

Structured file operations:

```json
{
  "operations": [
    {
      "type": "edit",
      "path": "src/a.rs",
      "edits": [
        {
          "old_string": "old",
          "new_string": "new"
        }
      ]
    },
    {
      "type": "create",
      "path": "src/new.rs",
      "content": "pub fn new_file() {}\n"
    },
    {
      "type": "delete",
      "path": "src/old.rs"
    },
    {
      "type": "move",
      "from": "src/name_old.rs",
      "to": "src/name_new.rs"
    }
  ],
  "cwd": "D:/path/to/workspace",
  "skillToken": "..."
}
```

## Fields

- `path` plus top-level `edits` keeps compatibility with the original single-file mode.
- `files` is for multiple existing files, each with its own `path` and `edits`.
- `operations` supports `edit`, `create`, `delete`, and `move`.
- `cwd` is required when more than one allowed directory is configured.
- `skillToken` is required for normal calls. The documentation read for this SKILL.md is the only call that does not require it.

Edit fields:

- `old_string` must be exact current file text after normalizing CRLF to LF.
- `new_string` is the replacement text.
- `replace_all` replaces every occurrence of `old_string` in the current in-memory state.
- `startLine` is optional and 1-based. Use it when the same `old_string` appears more than once and you want the closest match to that line.

Create and move fields:

- `content` is required for `create`.
- `overwrite` defaults to false. When false, `create` or `move` fails if the target already exists.
- `from` and `to` are required for `move`.

## Rules

1. Read relevant current content with `read_file` before editing unless the exact text is already in context. Use terminal reads only if `read_file` is disabled, unavailable, or unsuitable.
2. Use exact `old_string` text copied from the current file, including indentation. Do not include the line-number prefix or tab from `read_file` output.
3. Keep `old_string` as small as practical while still unique. If it is not unique, either set `replace_all` or provide `startLine`.
4. Do not set `old_string` equal to `new_string`.
5. Do not use an empty `old_string`; use `create` for new files, or include surrounding existing text for insertions.
6. Do not touch the same path more than once in one call. Put all replacements for one file into a single `edit` operation.
7. Order edits inside one file so a later `old_string` does not target text produced by an earlier `new_string`.
8. For TS, TSX, JS, and JSX template strings, write `${...}` exactly. Do not escape the dollar sign as `\${...}` unless the target source code truly needs a literal `${...}` string.

## Result And Events

On success, the result includes `added`, `modified`, `deleted`, `moved`, a compact `delta` with byte counts and affected path metadata, and a `warnings` array when the gateway detects likely syntax hazards such as unbalanced delimiters or accidental `\${...}` in TS/JS files. Warnings do not block the write; inspect and verify them before continuing.

The admin event stream records an `editPreview` event before policy confirmation and a `finished` event after completion.
