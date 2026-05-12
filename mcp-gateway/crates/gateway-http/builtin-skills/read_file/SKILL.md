---
name: read_file
description: >-
  Read text files through the gateway with line-numbered output, allowed-directory
  enforcement, binary-file rejection, and bounded line windows. Use this before
  editing files with multi_edit_file.
  First read the complete builtin://read_file/SKILL.md to get skillToken; this
  SKILL.md read does not require skillToken. Calls without the correct
  skillToken will fail and must be retried.
metadata:
  bundled: true
  tool: read_file
  category: filesystem
---

# Read File

Use this bundled skill to read text files from configured allowed directories. Prefer it over terminal commands such as `cat`, `Get-Content`, `type`, `sed`, or `head` when you need file contents for code understanding or manual edits.

The gateway returns stable, line-numbered text and records the exact path being read before any content is returned. This makes file reads easier to audit and safer to combine with `multi_edit_file`.

## Input

```json
{
  "path": "src/example.rs",
  "cwd": "D:/path/to/workspace",
  "offset": 1,
  "limit": 200,
  "skillToken": "..."
}
```

- `path` is the file to read. Relative paths resolve from `cwd`. Absolute paths are allowed only when they remain inside configured allowed directories.
- `cwd` is required when more than one allowed directory is configured.
- `offset` is the 1-based starting line. Omit it to start at line 1.
- `limit` is the number of lines to return. Omit it to read up to 2000 lines. The maximum is 2000.
- `skillToken` is required for normal file reads. The documentation read for this SKILL.md is the only call that does not require it.

## Output

Returned text uses:

```text
line_number<TAB>content
```

Example:

```text
41	fn handle_request() {
42	    route();
43	}
```

Do not include the line-number prefix or tab when copying `old_string` into `multi_edit_file`. The prefix is only for orientation and `startLine` hints.

The structured result includes:

- `path`
- `startLine`
- `endLine`
- `numLines`
- `totalLines`
- `truncated`
- `lineTruncated`
- `content`

## Rules

1. Use `read_file` before manually editing an existing file unless the exact current content is already in the conversation.
2. Read only the focused region you need. For large files, use `offset` and `limit` instead of reading the whole file repeatedly.
3. Use `offset` values from search results or compiler diagnostics to inspect nearby context.
4. If `multi_edit_file` reports an ambiguous or missing `old_string`, re-read the relevant region with `read_file` and copy the current text exactly.
5. Do not use terminal commands for ordinary text file reads unless `read_file` is disabled, unavailable, or unsuitable for the file type.
6. Use terminal commands for directory listings, generated command output, test/build output, and project tools. `read_file` reads files, not directories.

## Limits

- Text only. Binary files are rejected.
- UTF-8 text only. Files that cannot be decoded as UTF-8 are rejected.
- Maximum file size is 10 MiB.
- Maximum returned lines per call is 2000.
- Very long individual lines are truncated in output.
