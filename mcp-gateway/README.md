# MCP Gateway (V2)

This repository is the backend split-out for MCP Gateway.

## Workspace Layout

- `crates/gateway-core`: config model, config service, protocol/runtime process manager
- `crates/gateway-http`: Axum HTTP API, auth middleware, SSE bridge, OpenAPI generation
- `crates/gateway-cli`: CLI entry point (`gateway` binary)
- `crates/gateway-integration-tests`: black-box integration tests

## API Contract

- Base prefix: `/api/v2`
- Streamable HTTP: `POST /api/v2/mcp/{server_name}`
- SSE subscribe/request: `GET|POST /api/v2/sse/{server_name}`
- Admin API: `/api/v2/admin/*`
- OpenAPI: `/api/v2/openapi.json`
- Swagger UI: `/api/v2/docs`

All responses use envelope:

```json
{
  "ok": true,
  "data": {},
  "requestId": "uuid"
}
```

Error responses:

```json
{
  "ok": false,
  "error": {
    "code": "VALIDATION_FAILED",
    "message": "..."
  },
  "requestId": "uuid"
}
```

## Quick Start

```bash
cp config.example.json ./config.v2.json
cargo run -p gateway-cli -- run --config ./config.v2.json
```

## CLI

```bash
gateway run --config <path> --mode <extension|general|both> --listen <addr>
gateway init --config <path> --mode <extension|general|both>
gateway validate --config <path>
gateway token rotate --scope <admin|mcp> --config <path>
gateway migrate-config --from v1 --to v2 --input <old> --output <new>
```

## Quality Gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Skills MCP Usage

The gateway exposes two Skill MCP servers. External skills discovered from configured roots use `skills.serverName`; bundled tools use `skills.builtinServerName`.

- `POST /api/v2/mcp/{skills.serverName}`
- `GET|POST /api/v2/sse/{skills.serverName}`
- `POST /api/v2/mcp/{skills.builtinServerName}`
- `GET|POST /api/v2/sse/{skills.builtinServerName}`

Default external server name is `__skills__`; default built-in server name is `__builtin_skills__`. The two names must be different.

### Browser JSON-RPC Example

```ts
const gatewayBase = "http://127.0.0.1:8765";
const mcpToken = "YOUR_MCP_TOKEN";
const adminToken = "YOUR_ADMIN_TOKEN";
const skillsServer = "__skills__";
const builtinSkillsServer = "__builtin_skills__";

async function callSkillsMcp(payload: unknown) {
  const resp = await fetch(`${gatewayBase}/api/v2/mcp/${skillsServer}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${mcpToken}`,
    },
    body: JSON.stringify(payload),
  });
  return await resp.json();
}

// 1) list tools
const tools = await callSkillsMcp({
  jsonrpc: "2.0",
  id: "1",
  method: "tools/list",
  params: {},
});
console.log(tools.result.tools);

// 2) list built-in tools
const builtinTools = await fetch(`${gatewayBase}/api/v2/mcp/${builtinSkillsServer}`, {
  method: "POST",
  headers: {
    "Content-Type": "application/json",
    Authorization: `Bearer ${mcpToken}`,
  },
  body: JSON.stringify({
    jsonrpc: "2.0",
    id: "builtin-1",
    method: "tools/list",
    params: {},
  }),
}).then((resp) => resp.json());
console.log(builtinTools.result.tools);

// 3) run an external skill script
let runResponse = await callSkillsMcp({
  jsonrpc: "2.0",
  id: "2",
  method: "tools/call",
  params: {
    name: "skills_script_run",
    arguments: {
      skill: "ui-ux-pro-max",
      script: "scripts/search.py",
      args: ["--help"],
    },
  },
});

// 4) confirmation_required => admin approves
const content = runResponse.result?.structuredContent ?? {};
if (content.status === "confirmation_required") {
  const confirmationId = content.confirmationId as string;

  await fetch(
    `${gatewayBase}/api/v2/admin/skills/confirmations/${confirmationId}/approve`,
    {
      method: "POST",
      headers: {
        Accept: "application/json",
        Authorization: `Bearer ${adminToken}`,
      },
    },
  );

  // 5) retry with confirmationId
  runResponse = await callSkillsMcp({
    jsonrpc: "2.0",
    id: "3",
    method: "tools/call",
    params: {
      name: "skills_script_run",
      arguments: {
        skill: "ui-ux-pro-max",
        script: "scripts/search.py",
        args: ["--help"],
        confirmationId,
      },
    },
  });
}
```

### Policy Model

`skills.policy` now supports:

- command-tree rules (`rules`) with `allow|confirm|deny` actions
- directory whitelist guard (`pathGuard.whitelistDirs`)
- configurable violation behavior (`pathGuard.onViolation`)

This replaces broad keyword matching and reduces false positives/negatives.
