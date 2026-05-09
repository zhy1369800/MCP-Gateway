# Local MCP Gateway

[English](./README.md) | [中文](./README.zh.md)

Local MCP Gateway is a local gateway for MCP clients.

It exposes local capabilities as standard MCP `SSE` and `Streamable HTTP` endpoints, so other MCP clients can call them through one gateway. Upstream, the gateway can connect to normal stdio MCP servers, custom Skills, bundled tools, and desktop/browser automation tools.

In practice, it turns a local machine into a controllable MCP tool hub: clients can call tools to read files, edit files, run commands, operate a browser, or use your own Skill workflows while keeping configuration, auth, path guards, and approvals in one UI.

![Local MCP Gateway UI](./image.png)

## What It Does

- Exposes MCP services over `SSE` and `Streamable HTTP`
- Converts local stdio MCP servers into endpoints other clients can use
- Manages multiple MCP services from one UI
- Provides fixed MCP endpoints for external Skills and bundled Skills
- Includes built-in tools such as file reading, command execution, multi-file editing, browser control, and adapter debugging
- Supports path allowlists, command policies, execution limits, and manual approvals
- Keeps Skill usage token-light: Skills are described by small `SKILL.md` documents and only expanded when a client chooses to use them

## Endpoint Shape

For a configured MCP server named `<serverName>`:

```text
SSE:  http://<listenAddress>/api/v2/sse/<serverName>
HTTP: http://<listenAddress>/api/v2/mcp/<serverName>
```

Skill endpoints are fixed:

```text
External Skills: /api/v2/sse/__skills__
External Skills: /api/v2/mcp/__skills__
Bundled Skills:  /api/v2/sse/__builtin_skills__
Bundled Skills:  /api/v2/mcp/__builtin_skills__
```

If `MCP Token` is configured, clients should send:

```text
Authorization: Bearer <your_mcp_token>
```

## Typical Use

1. Open the app and set the listen address, for example `127.0.0.1:8765`.
2. Add local MCP servers, such as a filesystem or Playwright stdio server.
3. Enable the bundled tools or add external Skill directories.
4. Configure allowed directories and command confirmation rules.
5. Start the gateway.
6. Copy the generated `SSE` or `HTTP` endpoint into your MCP client.

Example endpoint:

```text
http://127.0.0.1:8765/api/v2/sse/playwright
```

## Skills And Built-In Tools

The gateway can expose two kinds of Skill MCP servers:

- `__skills__`: Skills discovered from directories you add. Each Skill is backed by a `SKILL.md`.
- `__builtin_skills__`: bundled practical tools shipped with the gateway.

Bundled tools currently include:

- `read_file`
- `shell_command`
- `multi_edit_file`
- `task-planning`
- `chrome-cdp`
- `chat-plus-adapter-debugger`

This makes it possible to build agent-like workflows on top of normal MCP clients: inspect a project, read documentation, edit code, run commands, test behavior, and control a browser, while still routing everything through MCP.

## Safety

Some Skills and built-in tools can execute commands, edit files, or control local applications. Use `Admin Token` and `MCP Token` when exposing the gateway beyond your own machine, and configure allowed directories, confirmation rules, and execution limits before enabling high-risk tools.

You are responsible for the consequences of commands and tools you approve.
