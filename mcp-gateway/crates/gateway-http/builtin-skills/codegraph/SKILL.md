---
name: codegraph
description: >-
  Run CodeGraph CLI commands through npx for local codebase indexing, semantic
  search, context building, and impact analysis. First read the complete
  builtin://codegraph/SKILL.md to get skillToken; this SKILL.md read does not
  require skillToken. Calls without the correct skillToken will fail and must
  be retried.
---

# CodeGraph

CodeGraph builds a local semantic graph of a codebase and exposes CLI commands
for indexing, searching symbols, building task context, listing files, and
checking affected code.

## IMPORTANT: How to Use This Tool

This is a **dedicated built-in tool** named `codegraph`. You MUST follow these
rules:

1. **Do NOT use `shell_command` to run CodeGraph commands.** The gateway blocks
   attempts to run `codegraph` or `npx -y @colbymchenry/codegraph` through
   `shell_command` when this tool is enabled. Always use this `codegraph` tool.

2. **Executing commands (skillToken required):**
   - Every `exec` value MUST start with the `codegraph` prefix.
   - You MUST include the `skillToken` returned from reading this document.
   - Example: `codegraph({"exec": "codegraph status", "cwd": "D:/project", "skillToken": "<token>"})`

3. **The gateway runs CodeGraph through npx.** You write `codegraph ...`, and
   the gateway executes `npx -y @colbymchenry/codegraph ...` directly without a
   shell wrapper.

4. **This is NOT the CodeGraph MCP server integration.** Do not run
   `codegraph serve --mcp`. This built-in tool only supports CLI commands.

## Prerequisites

Node.js/npm must be installed so `npx` is available. The first call may download
or refresh the `@colbymchenry/codegraph` npm package cache.

Always set `cwd` to the project root or a directory inside the configured
allowed directories. CodeGraph stores its local index under `.codegraph/`.

## Allowed Commands

Use these commands through the `codegraph` tool:

```bash
codegraph --version
codegraph help
codegraph init -i
codegraph index
codegraph sync
codegraph status
codegraph query <search>
codegraph files
codegraph context <task>
codegraph affected <files...>
```

Blocked commands:

```bash
codegraph serve --mcp
codegraph install
codegraph uninit
```

## Typical Workflow

Check whether the project is initialized:

```bash
codegraph({"exec": "codegraph status", "cwd": "D:/project", "skillToken": "<token>"})
```

Initialize and index a project when needed:

```bash
codegraph({"exec": "codegraph init -i", "cwd": "D:/project", "skillToken": "<token>"})
```

Search for a symbol:

```bash
codegraph({"exec": "codegraph query AuthService", "cwd": "D:/project", "skillToken": "<token>"})
```

Build task context:

```bash
codegraph({"exec": "codegraph context \"how authentication middleware works\"", "cwd": "D:/project", "skillToken": "<token>"})
```

Find affected tests or files:

```bash
codegraph({"exec": "codegraph affected src/auth.ts", "cwd": "D:/project", "skillToken": "<token>"})
```

## Notes

- Prefer `codegraph status` before assuming the project is indexed.
- Use `codegraph sync` after large file changes if the index looks stale.
- Do not use shell redirection or pipes with this tool. If you need to persist
  output, use `multi_edit_file` for files and `read_file` for inspection.
