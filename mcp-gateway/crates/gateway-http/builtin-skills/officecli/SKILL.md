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

## Prerequisites

The `officecli` binary must be installed and available in PATH before this tool can be enabled.
Use `officecli --version` to verify. If not installed, enable it via the admin UI or install manually
from https://github.com/iOfficeAI/OfficeCLI/releases.

---

## Strategy

**L1 (read) → L2 (DOM edit) → L3 (raw XML)**. Always prefer higher layers. Add `--json` for structured output.

## Help System

When unsure about property names, value formats, or command syntax, ALWAYS run help instead of guessing.

```bash
officecli help                                  # All commands + global options
officecli help docx                             # List all docx elements
officecli help docx paragraph                   # Full schema
officecli help docx set paragraph               # Verb-filtered
```

Format aliases: `word`→`docx`, `excel`→`xlsx`, `ppt`/`powerpoint`→`pptx`.

## Quick Start

**PPT:**
```bash
officecli create slides.pptx
officecli add slides.pptx / --type slide --prop title="Q4 Report"
officecli add slides.pptx '/slide[1]' --type shape --prop text="Revenue grew 25%"
```

**Word:**
```bash
officecli create report.docx
officecli add report.docx /body --type paragraph --prop text="Executive Summary" --prop style=Heading1
```

**Excel:**
```bash
officecli create data.xlsx
officecli set data.xlsx /Sheet1/A1 --prop value="Name" --prop bold=true
```

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

## Notes

- Paths are **1-based**: `'/body/p[3]'` = third paragraph
- After modifications, verify with `validate` and/or `view issues`
- When unsure, run `officecli help <format> <element>` instead of guessing
