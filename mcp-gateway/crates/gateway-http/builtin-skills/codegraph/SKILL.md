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
for indexing, symbol search, task context, file structure, call relationships,
impact analysis, and affected-test discovery.

Use this tool when a repository has or should have a `.codegraph/` index and the
task needs structural code intelligence: "where is this symbol", "how does this
area work", "who calls this", "what might break if I change it", or "which
tests are affected by these files".

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
   `codegraph serve --mcp`. This built-in tool only supports selected CLI
   commands. Upstream MCP-only tools such as `codegraph_trace`,
   `codegraph_explore`, and `codegraph_node` are not available through this
   built-in tool.

## Prerequisites

Node.js/npm must be installed so `npx` is available. The upstream npm package
currently requires a supported Node runtime, and the first call may download or
refresh the `@colbymchenry/codegraph` npm package cache.

Always set `cwd` to the project root or a directory inside the configured
allowed directories. CodeGraph stores its local SQLite index under
`.codegraph/`.

## Allowed Commands

Use these commands through the `codegraph` tool:

```bash
codegraph --version
codegraph help [subcommand]
codegraph init [path] -i
codegraph index [path]
codegraph sync [path]
codegraph status [path]
codegraph query <search> [--kind <kind>] [--limit <n>] [--json]
codegraph files [path] [--format <format>] [--filter <glob>] [--max-depth <n>] [--json]
codegraph context <task> [--max-nodes <n>] [--max-code <n>] [--no-code] [--format markdown|json]
codegraph callers <symbol> [--limit <n>] [--json]
codegraph callees <symbol> [--limit <n>] [--json]
codegraph impact <symbol> [--depth <n>] [--json]
codegraph affected <files...> [--depth <n>] [--filter <glob>] [--json] [--quiet]
```

Blocked commands:

```bash
codegraph serve --mcp
codegraph install
codegraph uninstall
codegraph uninit
codegraph unlock
```

Do not use the no-argument `codegraph` installer through this tool.

## Typical Workflow

Check whether the project is initialized:

```bash
codegraph({"exec": "codegraph status", "cwd": "D:/project", "skillToken": "<token>"})
```

Initialize and index a project when needed:

```bash
codegraph({"exec": "codegraph init -i", "cwd": "D:/project", "skillToken": "<token>"})
```

Search for a symbol or inspect a broad area:

```bash
codegraph({"exec": "codegraph query AuthService", "cwd": "D:/project", "skillToken": "<token>"})
codegraph({"exec": "codegraph context \"how authentication middleware works\" --max-nodes 30", "cwd": "D:/project", "skillToken": "<token>"})
```

Inspect call relationships:

```bash
codegraph({"exec": "codegraph callers AuthService --json", "cwd": "D:/project", "skillToken": "<token>"})
codegraph({"exec": "codegraph callees AuthService --limit 30", "cwd": "D:/project", "skillToken": "<token>"})
codegraph({"exec": "codegraph impact AuthService --depth 2", "cwd": "D:/project", "skillToken": "<token>"})
```

Find affected tests or files:

```bash
codegraph({"exec": "codegraph affected src/auth.ts", "cwd": "D:/project", "skillToken": "<token>"})
```

## Notes

- Prefer `codegraph status` before assuming the project is indexed.
- Use `codegraph sync` after large file changes if the index looks stale.
- Use `codegraph context` first for architecture or "how does this work"
  questions; use `query` for exact symbol lookup; use `callers`, `callees`, or
  `impact` for change planning.
- Prefer `--json` when you need structured output for follow-up reasoning.
- Do not use shell redirection, pipes, or `affected --stdin` with this tool. If
  you need affected tests, pass file paths as arguments. If you need to persist
  output, use `multi_edit_file` for files and `read_file` for inspection.
