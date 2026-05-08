---
name: chrome-cdp
description: Browser automation through chrome-devtools-axi, the AXI CLI wrapper for chrome-devtools-mcp
---

# Chrome CDP

This built-in skill uses `chrome-devtools-axi`. If `chrome-devtools-axi` is on
`PATH`, the gateway runs it directly. Otherwise it falls back to:

```bash
npx -y chrome-devtools-axi@latest <command>
```

The tool name remains `chrome-cdp` for compatibility, but commands and output
follow the AXI CLI documented at https://axi.md/.

## Default Browser Mode

By default, the gateway starts AXI in a headed persistent browser session
through `chrome-devtools-mcp`. A real Chrome window should appear for
`open <url>` or `start`, and the browser uses the same gateway-owned profile on
later runs.

- It does not attach to the user's already-open Chrome.
- It does not reuse `chrome://inspect` or an existing `9222` debugging port.
- It sets `CHROME_DEVTOOLS_AXI_HEADED=1`, so the browser is visible
  instead of headless.
- It clears AXI attach-related environment variables for built-in calls:
  `CHROME_DEVTOOLS_AXI_AUTO_CONNECT`, `CHROME_DEVTOOLS_AXI_BROWSER_URL`,
  and `CHROME_DEVTOOLS_AXI_WS_HEADERS`.
- It sets `CHROME_DEVTOOLS_AXI_USER_DATA_DIR` to a gateway-owned persistent
  Chrome profile directory. Cookies, cache, localStorage, and login sessions
  should be kept across `stop`, window close, gateway restart, and later opens
  unless the site expires the session or the profile directory is deleted.
- It sets `CHROME_DEVTOOLS_AXI_DISABLE_HOOKS=1` so the packaged CLI does not
  install agent session hooks.
- It uses a gateway-owned HOME/USERPROFILE directory for AXI state, so the
  bridge PID file does not collide with a user-started AXI session.

The gateway assigns `CHROME_DEVTOOLS_AXI_PORT` to a free localhost port before
starting AXI. This avoids conflicts with another bridge or another tool already
using AXI's default bridge port.

The gateway also gives built-in `chrome-cdp` calls a longer default timeout
because the first run may need `npx` package resolution plus Chrome startup.

## Cache and Login State

Default behavior is to reuse the existing browser cache/profile. Do not create
a fresh profile just because a new task starts, a new page is opened, or the
browser was closed and reopened.

Use the default persistent profile when the user asks to:

- open a page
- continue browsing
- debug a site
- use an already logged-in session
- reopen the browser later

Only use a new cache/profile when the user explicitly asks for a clean browser,
fresh cache, separate login, separate account, temporary session, or similar.
In that case, use a different `CHROME_DEVTOOLS_AXI_USER_DATA_DIR` value for the
gateway process or add a future profile-selection option before launching AXI.
Do not delete or overwrite the default persistent profile unless the user
explicitly asks to clear saved browser data.

## Port Handling

There are two different ports:

- AXI bridge port: the local HTTP bridge used by `chrome-devtools-axi`.
- Chrome debugging port: managed by `chrome-devtools-mcp` for the persistent
  browser it launches.

If the user already has Chrome DevTools remote debugging open, keep using this
skill normally. The built-in default launches a separate persistent-profile
browser and lets the underlying MCP/browser tooling choose its own debugging
endpoint.

Only use an existing browser when a future workflow explicitly opts into that
mode outside this built-in tool.

## Commands

You may pass either a full AXI command:

```bash
npx -y chrome-devtools-axi@latest open https://example.com
```

or the short form accepted by the gateway:

```bash
open https://example.com
snapshot
pages
click @12
eval "document.title"
stop
```

## Navigation

```bash
open <url>          # navigate and return a snapshot
snapshot            # capture current page state
screenshot <path>   # save a screenshot
scroll <dir>        # up, down, top, bottom
back
wait <ms|text>
eval <js>
```

`eval` wraps plain input as an expression. For multi-statement JavaScript, pass
an arrow function or function expression, not an already-invoked IIFE.

When calling this tool through MCP Gateway on Windows, quote the whole
JavaScript function as one argument so the command parser and `npx.cmd` do not
split or strip JavaScript string literals:

```bash
eval "function(){ return document.title }"
eval "() => document.title"
```

Avoid these forms for multi-statement snippets:

```bash
eval (() => { return document.title })()
eval function(){ const x = 'abc def'; return x }
```

The first form is treated as a non-function by AXI, and the second form is split
into many shell tokens before it reaches AXI.

## Interaction

```bash
click @<uid>
fill @<uid> <text>
type <text>
press <key>
hover @<uid>
drag @<from> @<to>
fillform @<uid>=<value> ...
dialog <accept|dismiss>
upload @<uid> <path>
```

Use `snapshot` first to obtain `uid=` references, then pass them as `@<uid>`.

## Page Management

```bash
pages
newpage <url>
selectpage <id>
closepage <id>
resize <width> <height>
```

## Debugging Data

```bash
console
console-get <id>
network
network --type <type>
network --type <type> --limit <n> --page <n>
network-get <id-or-url>
network-get <id-or-url> --full
network-get <id-or-url> --request-file "<abs-path>" --response-file "<abs-path>"
```

Use `network-get` for the candidate request that was produced by the current
user action. Do not treat a URL by itself as enough evidence for a site adapter:
the adapter needs the real request shape and the real response shape.

Fast path for request body / response body capture:

1. Use page `eval` or the workflow skill to record a Performance baseline before
   the user action.
2. After the user action, read new `PerformanceResourceTiming` entries to get
   candidate URLs and timing.
3. Use `network --type fetch --limit 20 --page 0` first for modern chat apps.
   If the output says there is another page, run `network --type fetch --limit
   20 --page 1`, then continue page by page. Use `xhr` or `websocket` only when
   Performance or page behavior points to those transports.
4. Match by URL, method, status, timing, and resource type. Copy the `reqid`
   from the `network` output.
5. Use `network-get <reqid> --request-file "<abs-path>" --response-file
   "<abs-path>"` to save the complete bodies.
6. List the output directory and read the actual generated files. AXI may append
   `.network-request` and `.network-response` to the names.

When the URL is known but the request body or response body is missing:

- First verify that the selected entry is the real chat request, not an
  `OPTIONS` preflight, analytics request, config fetch, redirect, heartbeat, or
  title/history update.
- Prefer the exact request id from `network` when available. `network-get <url>`
  is not guaranteed to find a request from a Performance URL; if URL lookup
  returns no selected request, use `network --type fetch --limit <n> --page <n>`
  or the relevant request type to locate the `reqid`.
- `network --page` is zero-based. If output says `Next page: 1`, rerun with
  `--page 1`.
- If normal output is truncated or body fields are omitted, rerun with
  `--request-file` and `--response-file`, then inspect the saved files with a
  filesystem tool. `--full` can help with screen output, but saved files are the
  preferred way to capture long JSON, SSE, NDJSON, or streamed text.
- AXI may append `.network-request` and `.network-response` suffixes to the
  requested file names. After saving, list the output directory and read the
  actual files that were created.
- On Windows, quote absolute file paths passed to `--request-file` and
  `--response-file`.
- If the body file is empty, classify why before moving on: no request body,
  preflight request, response still streaming, opaque/binary/protobuf body,
  WebSocket transport, worker/service-worker transport, redirect, or a tool
  capture limitation.
- For WebSocket traffic, inspect WebSocket entries and frames instead of trying
  to force `network-get` to return an HTTP response body.
- Never print or summarize sensitive headers such as `Authorization`, `Cookie`,
  `Set-Cookie`, device ids, or session ids in the final answer.

## Performance

```bash
lighthouse
perf-start
perf-stop
perf-insight <set> <name>
heap <path>
```

## Bridge Lifecycle

```bash
start
stop
```

`start` launches the persistent AXI bridge and persistent-profile browser if
needed.
`stop` stops the bridge and its child browser processes.

Process cleanup is handled by the gateway process lifecycle. On Windows, child
processes launched by the gateway are attached to a gateway-owned Job Object so
they are terminated when the gateway exits.

## Tips

- Prefer `open <url>` or `snapshot` before clicking or filling so the output
  contains current `uid=` refs.
- Prefer `network` and `network-get` for request debugging instead of raw CDP
  calls.
- If startup is slow because `npx` is cold, install packages globally:
  `npm install -g chrome-devtools-axi chrome-devtools-mcp`. The gateway will
  use the global `chrome-devtools-axi` binary when it is on `PATH`.
- For the fastest MCP startup, set `CHROME_DEVTOOLS_AXI_MCP_PATH` to the global
  `chrome-devtools-mcp` entrypoint for the gateway process.
- If AXI output is truncated, rerun the command with `--full`.
