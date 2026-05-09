# Local MCP Gateway (Latest)

[English](./README.md) | [中文](./README.zh.md)

MCP Gateway is an MCP (Model Context Protocol) server gateway.  
It unifies multiple MCP servers behind one entry point, and provides proxy forwarding, authentication, admin APIs, plus the new `SKILLS` capability.

Common use case: convert local `stdio` MCP services into remotely accessible `SSE / Streamable HTTP` MCP services, so desktop or browser AI clients can use tools and skills in web chat interfaces.

## Overview

- Manage multiple MCP services in one place (Visual + JSON editing modes)
- Unified `SSE` forwarding: default `GET|POST /api/v2/sse/<serverName>`
- Unified `HTTP` forwarding: default `POST /api/v2/mcp/<serverName>`
- Built-in authentication (`Admin Token` / `MCP Token`)
- Built-in Skill MCP management in the `SKILLS` tab
- Bundled Skills: `read_file`, `shell_command`, `multi_edit_file`, `chrome-cdp`, and `chat-plus-adapter-debugger`, each individually toggleable
- External Skill root management with per-root enable switches and `SKILL.md` validation
- Access boundary / path guard (allowed directories + out-of-scope policy)
- Execution limits (timeout, max output)
- Visual policy rule manager (`deny / confirm`, search, add, edit, copy, delete)
- Pending command approval (`Approve / Reject`) with confirmation popup

## UI Preview

### Image one (Main MCP configuration)

![Local MCP Gateway Main UI](./image.png)

### Image two (SKILLS setup, bundled skills, and external roots)

![Image two](./image2.png)

The latest SKILLS setup screen shows the external and built-in Skill server names, bundled Skills, and external Skill roots. Each external root can be browsed, validated, enabled or disabled, and removed independently.

### Image three (Visual policy rule manager)

![Image three](./image3.png)

The latest policy screen replaces the old rules-only explanation with a visual manager. Rules are grouped by action, searchable by command, keyword, or reason, and can be added, edited, copied, or deleted from the UI.

## 1. MCP Tab Configuration

### Gateway Settings

- `Listen Address`: gateway listen address and port, e.g. `127.0.0.1:8765`
- `SSE Path`: default `"/api/v2/sse"`
- `HTTP Stream Path`: default `"/api/v2/mcp"`

Final endpoint rule:

- `SSE`: `http://<listenAddress><ssePath>/<serverName>`
- `HTTP`: `http://<listenAddress><httpPath>/<serverName>`

Example (listen on `127.0.0.1:8765`):

- `http://127.0.0.1:8765/api/v2/sse/filesystem`
- `http://127.0.0.1:8765/api/v2/mcp/filesystem`

### Security (Password / Token)

- `ADMIN TOKEN`: protects `/api/v2/admin/*`
- `MCP TOKEN`: protects `/api/v2/mcp/*` and `/api/v2/sse/*`

Notes:

- In the current UI, leaving token empty disables that auth scope
- For public exposure, enable auth and use long random tokens (as gateway passwords)
- Client requests should include header: `Authorization: Bearer <your_token>`

### MCP Service List

Each row is one MCP service:

- Toggle: enable/disable the service
- `Name`: service name (used in URL suffix)
- `Command`: startup command (e.g. `npx`)
- `Args`: command arguments
- `+`: add environment variables
- `x`: remove service

Example (Playwright MCP):

1. Name: `playwright`
2. Command: `npx`
3. Args: `-y @playwright/mcp@latest`

## 2. New SKILLS Feature

The `SKILLS` tab is used to enable and manage the built-in Skill MCP service:

1. Set `External Skill Server Name` (default `__skills__`) and `Built-in Skill Server Name` (default `__builtin_skills__`). These two names must be different.
2. Review bundled Skills: `read_file`, `shell_command`, `multi_edit_file`, `chrome-cdp`, and `chat-plus-adapter-debugger`. Each can be toggled individually. Prefer `read_file` for source/config file reads before editing; it enforces allowed directories, returns line-numbered windows, and works with the editing tools.
3. Add `External Skill Roots`, validate that `SKILL.md` exists directly in each directory, and enable only the roots you want to expose.
4. Configure `Allowed Directories`. Commands and file edits must stay inside the allowed directories unless the selected violation action says otherwise.
6. Choose the violation action: `allow / confirm / deny`.
7. Configure execution limits: `Execution Timeout (ms)` (minimum `1000`) and `Max Output (bytes)` (minimum `1024`).
8. Manage policy rules in the visual rule manager. Rules support `deny` and `confirm`, command-prefix matching, keyword matching, search, add, edit, copy, and delete. The advanced JSON editor remains available for bulk paste or manual migration.
9. After running, approve or reject high-risk commands in `Pending Confirmations` or from the confirmation popup.

When the gateway is running, the UI shows external and built-in Skill endpoints:

- `External Skill SSE`: `http://<listenAddress><ssePath>/<skillsServerName>`
- `External Skill HTTP`: `http://<listenAddress><httpPath>/<skillsServerName>`
- `Built-in Skill SSE`: `http://<listenAddress><ssePath>/<builtinServerName>`
- `Built-in Skill HTTP`: `http://<listenAddress><httpPath>/<builtinServerName>`

## 3. Recommended Workflow

1. Configure listen address and paths in the `MCP` tab.
2. Set `ADMIN TOKEN` and `MCP TOKEN` as needed (recommended for production).
3. Add MCP services and save config.
4. Open the `SKILLS` tab and configure Skill capabilities (optional).
5. Click `Start` at top-right, and wait for running status.
6. Copy generated `SSE / HTTP` endpoints to your MCP client.

## 4. Visual / JSON Editing

- `Visual`: form-based editing for daily use
- `JSON`: direct edit of `mcpServers` object

You can switch between them. If JSON is invalid, UI will show an error and block startup.

## 5. Config File Location

The current config file path is shown at the bottom of the UI. Default paths are usually:

- Windows: `%APPDATA%\\mcp-gateway\\config.v2.json`
- macOS: `~/Library/Application Support/mcp-gateway/config.v2.json`
- Linux: `~/.config/mcp-gateway/config.v2.json`

## 6. FAQ

1. Startup failed  
Check each service has at least `Name` + `Command`.
2. Port already in use  
Change listen port (e.g. `127.0.0.1:9876`) and retry.
3. Client cannot connect  
Check service enabled status and verify URL path/service name.
4. SKILLS root cannot be enabled  
Ensure `SKILL.md` exists directly under the selected directory (current check is non-recursive).

## 7. Disclaimer

- This software provides `SKILLS` capabilities that may execute system commands or scripts with your authorization.
- Although command rules, path guards, and confirmation workflows are built in, they cannot guarantee complete coverage of all scenarios or absolute safety.
- Any consequences caused by using `SKILLS` or command execution (including but not limited to data loss, system issues, file corruption, service interruption, or hardware/software damage) are the sole responsibility of the user.
- The author and maintainers of this software are not liable for any direct, indirect, incidental, or consequential damages arising from such use.
- You should validate high-risk commands in a controlled environment and maintain proper backups and permission isolation.
