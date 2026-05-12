---
name: officecli
description: >-
  Create, analyze, proofread, and modify Office documents (.docx, .xlsx, .pptx)
  using the officecli CLI tool. First read the complete
  builtin://officecli/SKILL.md to get skillToken; this SKILL.md read does not
  require skillToken. Calls without the correct skillToken will fail and must
  be retried.
---

# officecli

AI-friendly CLI for .docx, .xlsx, .pptx. Single binary, no dependencies, no Office installation needed.

## IMPORTANT: How to Use This Tool

This is a **dedicated built-in tool** named `officecli`. You MUST follow these rules:

1. **Do NOT use `shell_command` to run officecli commands.** The gateway will block any attempt to run `officecli` through `shell_command`. Always use this `officecli` tool directly.

2. **Executing commands (skillToken required):**
   - Every `exec` value MUST start with the `officecli` prefix.
   - You MUST include the `skillToken` returned from step 2.
   - Example: `officecli({"exec": "officecli create report.docx", "skillToken": "<token>"})`

3. **The `exec` field always requires the `officecli` command prefix.** Do NOT omit it.
   - ✅ Correct: `"exec": "officecli help"`
   - ✅ Correct: `"exec": "officecli create file.docx"`
   - ❌ Wrong: `"exec": "help"`
   - ❌ Wrong: `"exec": "create file.docx"`

## Prerequisites

The `officecli` binary must be installed and available in PATH before this tool can be enabled.
Use `officecli --version` to verify. If not installed, enable it via the admin UI or install manually
from https://github.com/iOfficeAI/OfficeCLI/releases.

---

## Strategy

**L1 (read) → L2 (DOM edit) → L3 (raw XML)**. Always prefer higher layers. Add `--json` for structured output.

## Help System

When unsure about property names, value formats, or command syntax, ALWAYS run help instead of guessing.

```
officecli({"exec": "officecli help", "skillToken": "<token>"})
officecli({"exec": "officecli help docx", "skillToken": "<token>"})
officecli({"exec": "officecli help docx paragraph", "skillToken": "<token>"})
officecli({"exec": "officecli help docx set paragraph", "skillToken": "<token>"})
```

Format aliases: `word`→`docx`, `excel`→`xlsx`, `ppt`/`powerpoint`→`pptx`.

## Quick Start

**PPT:**
```
officecli({"exec": "officecli create slides.pptx", "skillToken": "<token>"})
officecli({"exec": "officecli add slides.pptx / --type slide --prop title=\"Q4 Report\"", "skillToken": "<token>"})
officecli({"exec": "officecli add slides.pptx '/slide[1]' --type shape --prop text=\"Revenue grew 25%\"", "skillToken": "<token>"})
```

**Word:**
```
officecli({"exec": "officecli create report.docx", "skillToken": "<token>"})
officecli({"exec": "officecli add report.docx /body --type paragraph --prop text=\"Executive Summary\" --prop style=Heading1", "skillToken": "<token>"})
```

**Excel:**
```
officecli({"exec": "officecli create data.xlsx", "skillToken": "<token>"})
officecli({"exec": "officecli set data.xlsx /Sheet1/A1 --prop value=\"Name\" --prop bold=true", "skillToken": "<token>"})
```

## REQUIRED: WPS Compatibility Fix

OfficeCLI writes `<Application>OfficeCLI/x.x.x</Application>` into `docProps/app.xml` when creating files. WPS Office does not recognize this application identifier and will refuse to open the file ("format not recognized").

**After every `officecli create`, you MUST run this cleanup step:**

```bash
officecli raw-set <file> docProps/app.xml --xpath "//ap:Application" --action delete
```

Example full flow:
```
officecli({"exec": "officecli create report.docx", "skillToken": "<token>"})
officecli({"exec": "officecli raw-set report.docx docProps/app.xml --xpath \"//ap:Application\" --action delete", "skillToken": "<token>"})
```

This applies to all formats (.docx, .xlsx, .pptx). Do NOT skip this step.

---

## L1: Create, Read & Inspect

```bash
officecli create <file>               # Create blank .docx/.xlsx/.pptx
officecli view <file> <mode>          # outline | stats | issues | text | annotated | html
officecli get <file> <path> --depth N # Get a node and its children [--json]
officecli query <file> <selector>     # CSS-like query
officecli validate <file>             # Validate against OpenXML schema
```

## L2: DOM Operations

### set — modify properties
```bash
officecli set <file> <path> --prop key=value [--prop ...]
```

### add — add elements
```bash
officecli add <file> <parent> --type <type> [--prop ...]
```

### remove
```bash
officecli remove <file> '/body/p[4]'
```

## L3: Raw XML (use when L2 cannot express what you need)

```bash
officecli raw <file> <part>
officecli raw-set <file> <part> --xpath "..." --action replace --xml '...'
```

## Common Pitfalls

| Pitfall | Correct Approach |
|---------|-----------------|
| Guessing property names | Run `officecli help <format> <element>` |
| Modifying an open file | Close the file in Office/WPS first |
| Using `shell_command` for officecli | Use this `officecli` tool directly |
| Omitting `officecli` prefix in exec | Always write `"exec": "officecli ..."` |

## Notes

- Paths are **1-based**: `'/body/p[3]'` = third paragraph
- After modifications, verify with `validate` and/or `view issues`
- When unsure, run `officecli help <format> <element>` instead of guessing

## Companion Tools

The `officecli` skill only owns `.docx` / `.xlsx` / `.pptx` documents. Any
secondary files (Markdown notes, JSON dumps from `officecli view --json`, unit
fixtures, extracted plain text) must go through the other bundled skills:

- Persist or rewrite secondary files with `multi_edit_file` so the gateway has
  a reviewable diff and path allowlist enforcement; do not use
  `shell_command` redirection, `Set-Content`, or here-documents for that.
- Inspect the current contents of those files with `read_file` so output is
  line-numbered and size-capped; do not use `shell_command` with `cat`,
  `Get-Content`, `type`, `sed`, `head`, or `tail`.
- Never wrap an `officecli` invocation inside `shell_command`. The gateway
  rejects that path. Call this `officecli` tool directly.
