---
name: apply_patch
description: >-
  Apply structured file additions, updates, deletes, and moves using narrow
  line-level diff hunks. In update hunks, every non-marker line must start with
  one diff prefix: space for unchanged context, - for removed lines, or + for
  added lines; never paste bare replacement code. First read the complete
  builtin://apply_patch/SKILL.md to get skillToken; this SKILL.md read does not
  require skillToken. Do not use regex or partial reads to fetch only the token.
  Calls without the correct skillToken will fail and must be retried, so get the
  token before patching.
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

## Mandatory Workflow

1. Know the current file text before editing. If the exact target lines are not already in context, use `shell_command` to read a focused region first.
2. Choose the smallest patch that expresses the requested change. Prefer one or two line replacements over replacing a whole function, array, or repeated rule list.
3. For `*** Update File`, write a line-level diff. Every code line in the hunk must start with one of: space, `-`, `+`.
4. Use exact `-` lines copied from the current file. Do not invent old lines from memory.
5. If the patch fails with `failed to find expected lines`, re-read the relevant file region and retry with a smaller, more accurate hunk. Do not retry by pasting a larger raw code block.

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

A single patch may contain multiple file operations, including updates to more than one file. Keep the combined patch reviewable. For large mechanical migrations across many repeated entries or structured files, prefer a narrowly scoped project script through `shell_command`, then verify with tests and inspect `git diff`.

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

The diff prefix is separate from source-code indentation. To preserve an indented unchanged line, put one leading space for the patch prefix, then the original indentation. To remove or add an indented line, put `-` or `+`, then the original indentation.

Every non-marker line inside `*** Update File` must be a line-level diff line. Do not paste raw replacement code. A code line without a leading space, `-`, or `+` is invalid. For example, this is wrong because the `SkillCommandRule` lines are bare code:

```text
*** Begin Patch
*** Update File: src/config.rs
@@ fn default_rules
SkillCommandRule {
id: "deny-diskpart".to_string(),
action: SkillPolicyAction::Confirm,
}
*** End Patch
```

Write the smallest exact change instead:

```text
*** Begin Patch
*** Update File: src/config.rs
@@ deny-diskpart
-            action: SkillPolicyAction::Deny,
+            action: SkillPolicyAction::Confirm,
@@ deny-diskpart
-            reason: "Disk partition command is blocked".to_string(),
+            reason: "Disk partition command requires confirmation".to_string(),
*** End Patch
```

For repeated structures such as config rule arrays, use a unique nearby identifier as the `@@` context and change only the fields that actually differ. Do not include dozens of neighboring entries just to edit one entry.

`*** Update File` must contain at least one non-empty hunk. Empty update sections are rejected.

The gateway accepts patch text wrapped in a simple heredoc envelope such as `<<'EOF' ... EOF` and strips the envelope before parsing. This is only for compatibility with shell-shaped calls; when using the `apply_patch` tool directly, send the patch body itself.

When locating update hunks, the gateway tries exact matching first, then progressively allows trailing whitespace differences, surrounding whitespace differences, and common Unicode punctuation differences such as typographic dashes and quotes. Use enough context anyway; fuzzy matching is a recovery aid, not a replacement for precise patches.

If an update fails with `failed to find expected lines`, the old `-` and unchanged context lines did not appear as one contiguous sequence in the current file after the optional `@@` context. Re-read the relevant file region and retry with a smaller hunk or more accurate context. Do not respond by pasting a larger raw code block.

This tool does not accept standard unified diff headers such as `--- file` and `+++ file`. It also does not accept prose "search/replace" blocks or bare replacement code. Use only the grammar above.

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

Repeated rule entry update:

```text
*** Begin Patch
*** Update File: src/config.rs
@@ deny-diskpart
-            action: SkillPolicyAction::Deny,
+            action: SkillPolicyAction::Confirm,
@@ deny-diskpart
-            reason: "Disk partition command is blocked".to_string(),
+            reason: "Disk partition command requires confirmation".to_string(),
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

On success, the tool result includes a short summary and a compact `delta` with paths, change kinds, byte counts, and whether overwritten content existed. The tool result intentionally does not return full `oldContent` or `newContent` text, because large file contents waste model context. On failure, the result still includes a compact committed delta summary so callers can see whether any earlier file operations were already written. The `delta.exact` flag is `false` when the gateway cannot fully prove the recorded delta, for example after a failed write, unreadable overwritten content, or non-regular file behavior.

The admin event stream at `/api/v2/admin/skills/events?after=<seq>` records patch lifecycle events:

- `patchPreview` after parsing and before policy confirmation.
- `finished` with final status and committed delta.

## Editing Practice

- Keep patches narrow and related to the user's request.
- A patch may update multiple files, but do not make one huge patch just to batch unrelated or mechanical changes.
- Prefer changing only the specific lines needed. Do not replace a whole function, array, or block when one or two fields or statements are the real change.
- Preserve unrelated user changes in the worktree.
- Include enough unchanged context for the target location to be unambiguous.
- Use `@@ identifier` to jump near the target, then use exact `-` lines copied from the current file and `+` lines for the replacement.
- For insertions that are not at end of file, include at least one stable unchanged context line near the insertion point.
- For repeated blocks, avoid broad replacements. Target a unique label, id, function name, key, or nearby literal.
- Do not mix unrelated refactors into a behavior fix.
- After applying a patch, run targeted verification with `shell_command` when practical.
- After applying meaningful edits, inspect the relevant diff with `git diff -- path` or `git diff --stat` before reporting completion.
