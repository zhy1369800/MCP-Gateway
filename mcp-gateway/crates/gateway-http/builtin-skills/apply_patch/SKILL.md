---
name: apply_patch
description: Apply structured file additions, updates, deletes, and moves inside an allowed workspace.
metadata:
  bundled: true
  tool: apply_patch
  category: editing
---

# Apply Patch

Use this bundled skill for manual source edits when the desired change can be expressed as a structured patch. It is preferred over shell redirection because the gateway can parse affected paths before writing files and can apply path policy consistently.

## When To Use

- Add a new file with known content.
- Delete a file the user explicitly wants removed.
- Update a focused region of an existing file.
- Move a file while updating its content.
- Keep edits small enough that the before/after intent is obvious.

Use a formatter or project generator instead when the output is mechanical and owned by project tooling. Use `shell_command` for discovery, tests, builds, and generated commands.

## Scope And Cwd

- Set `cwd` to the concrete directory that relative patch paths should resolve from.
- `cwd` must be inside one configured allowed directory.
- Every affected file must remain inside configured allowed directories.
- If multiple allowed directories are configured and the user did not specify the target workspace, ask which directory should be used before applying a patch.
- Prefer relative paths from `cwd` unless an absolute path is clearer and allowed.

## Patch Grammar

Every patch starts with:

```text
*** Begin Patch
```

Every patch ends with:

```text
*** End Patch
```

Supported file operations:

```text
*** Add File: path/to/file
```

```text
*** Delete File: path/to/file
```

```text
*** Update File: path/to/file
```

Optional move line immediately after an update header:

```text
*** Move to: path/to/new-file
```

Inside an update hunk:

- `@@` starts a hunk.
- `@@ some context` starts a hunk and asks the gateway to first seek that context line, such as a function or class name, before locating the changed lines.
- Lines beginning with one space are unchanged context.
- Lines beginning with `-` are removed.
- Lines beginning with `+` are added.
- `*** End of File` after hunk lines anchors the hunk at the end of the file, which is useful for appends and tail edits.

`*** Update File` must contain at least one non-empty hunk. Empty update sections are rejected.

The gateway accepts patch text wrapped in a simple heredoc envelope such as `<<'EOF' ... EOF` and strips the envelope before parsing. This is only for compatibility with shell-shaped calls; when using the `apply_patch` tool directly, send the patch body itself.

When locating update hunks, the gateway tries exact matching first, then progressively allows trailing whitespace differences, surrounding whitespace differences, and common Unicode punctuation differences such as typographic dashes and quotes. Use enough context anyway; fuzzy matching is a recovery aid, not a replacement for precise patches.

This tool does not accept standard unified diff headers such as `--- file` and `+++ file`. It also does not accept prose "search/replace" blocks. Use only the grammar above.

## Examples

Minimal replacement:

```text
*** Begin Patch
*** Update File: index.html
@@
 <main>
-  <h1>Old title</h1>
+  <h1>New title</h1>
 </main>
*** End Patch
```

Add a file:

```text
*** Begin Patch
*** Add File: notes.txt
+first line
+second line
*** End Patch
```

Delete a file:

```text
*** Begin Patch
*** Delete File: obsolete.txt
*** End Patch
```

Move and update a file:

```text
*** Begin Patch
*** Update File: old-name.txt
*** Move to: new-name.txt
@@
-old content
+new content
*** End Patch
```

Context-targeted update:

```text
*** Begin Patch
*** Update File: src/app.rs
@@ fn render_title
-    "Old title"
+    "New title"
*** End Patch
```

Append at end of file:

```text
*** Begin Patch
*** Update File: CHANGELOG.md
@@
+- Added gateway apply_patch delta reporting.
*** End of File
*** End Patch
```

## Result And Events

On success, the result includes a summary plus a committed `delta` describing added, deleted, and updated file content. On failure, the result still includes the committed delta so callers can see whether any earlier file operations were already written. The `delta.exact` flag is `false` when the gateway cannot fully prove the recorded delta, for example after a failed write, unreadable overwritten content, or non-regular file behavior.

The admin event stream at `/api/v2/admin/skills/events?after=<seq>` records patch lifecycle events:

- `patchPreview` after parsing and before policy confirmation.
- `finished` with final status and committed delta.

## Editing Practice

- Keep patches narrow and related to the user's request.
- Preserve unrelated user changes in the worktree.
- Include enough unchanged context for the target location to be unambiguous.
- Do not mix unrelated refactors into a behavior fix.
- After applying a patch, run targeted verification with `shell_command` when practical.
