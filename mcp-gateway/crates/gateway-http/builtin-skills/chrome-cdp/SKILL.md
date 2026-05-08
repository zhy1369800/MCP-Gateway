---
name: chrome-cdp
description: >-
  Browser automation and debugging through bundled Chrome DevTools Protocol.
  First read the complete builtin://chrome-cdp/SKILL.md to get skillToken; this
  SKILL.md read does not require skillToken. Do not use regex or partial reads
  to fetch only the token. Later calls without the correct skillToken will fail
  and must be retried, so get the token before debugging.
---

# Chrome CDP

This built-in skill uses MCP Gateway's bundled `cdp.mjs` runner. It talks
directly to Chrome DevTools Protocol over WebSocket with no third-party browser
automation bridge.

## Default Browser Mode

By default, the gateway starts or reuses a visible Chromium-family browser in
managed launch mode.

- It does not attach to the user's already-open Chrome unless the gateway
  process is started with `CDP_BROWSER_MODE=attach`.
- It uses a gateway-managed runtime directory for target caches, per-tab
  daemons, and CDP state.
- It sets `CDP_PROFILE_MODE=persistent` by default.
- It sets `CDP_USER_DATA_DIR` to the gateway-owned persistent profile path:
  `mcp-gateway/builtin-skills/chrome-cdp/chrome-user-data` under the OS local
  data directory.
- Cookies, cache, localStorage, and login sessions are reused across commands,
  browser restarts, and gateway restarts unless the site expires them or the
  profile directory is deleted.

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
In that case, start the gateway with a different `CDP_USER_DATA_DIR`, or set
`CDP_PROFILE_MODE=empty` for a blank managed profile. Do not delete or overwrite
the default persistent profile unless the user explicitly asks to clear saved
browser data.

## Commands

Short form accepted by the gateway:

```bash
open https://example.com
list
snap
eval "document.title"
netclear
net api/chat
netget 1234.56 --request-file "D:\tmp\chat-request.txt" --response-file "D:\tmp\chat-response.txt"
stop
```

You may also pass the script form; the gateway still runs its bundled script:

```bash
node cdp.mjs open https://example.com
```

## Targets

Page-scoped commands can take a target prefix from `list`.

```bash
list
snap <target>
html <target> ".message-list"
click <target> "button[type=submit]"
```

For convenience, `open`, `launch`, and `list` maintain an active target. These
commands may omit the target and use the active page:

```bash
snap
eval "document.title"
netclear
net chat
netget <request-id>
```

If multiple pages are open and no active target is known, run `list` and pass
the target prefix explicitly.

## Navigation and DOM

```bash
launch [url]                    # start or reuse managed browser
open [url]                      # open a tab and select it as active
list                            # list open pages and target prefixes
snap [target]                   # accessibility tree snapshot
html [target] [selector]        # full page or element HTML
nav <target> <url>              # navigate target to URL
eval [target] <js>              # evaluate JavaScript in the page
evalraw <target> <method> [json]# raw CDP command passthrough
```

`eval` accepts expressions or function/IIFE source. When calling this tool
through MCP Gateway on Windows, quote the whole JavaScript expression as one
argument.

## Interaction

```bash
click <target> <selector>       # CSS selector
clickxy <target> <x> <y>        # CSS pixel coordinates
type <target> <text>            # Input.insertText at current focus
loadall <target> <selector> [ms]
shot [target] [file]
```

Use `type` after focusing an input with `click` or `clickxy`. It uses CDP input
events and works better than DOM assignment for cross-origin iframes.

## Network Debugging

Use the CDP Network recorder for searchable request capture.

```bash
netclear [target]               # enable CDP Network and clear captured events
net [target] [filter]           # list/search captured requests
netget [target] <request-id> [--full]
netget [target] <request-id> --request-file "<abs-path>" --response-file "<abs-path>"
perfnet [target]                # PerformanceResourceTiming fallback
```

Recommended request capture flow:

1. Open or select the target page.
2. Run `netclear`.
3. Perform the user action that sends the request.
4. Run `net <filter>` with a URL, method, endpoint fragment, MIME type, or
   keyword.
5. Copy the request id prefix from `net`.
6. Run `netget <request-id> --request-file "<abs-path>" --response-file "<abs-path>"`
   when full bodies are needed.

`netget` redacts sensitive headers such as `Authorization`, `Cookie`, and
`Set-Cookie` in its JSON output. Never print or summarize sensitive headers,
tokens, device ids, or session ids in the final answer.

If no CDP record appears, run `perfnet` to inspect browser resource timing, then
repeat with `netclear` before the next user action. Network capture only records
events after it has been enabled.

For WebSocket traffic, `netget` returns captured frames for the selected
WebSocket request id.

## Raw CDP

Use `evalraw` for precise DevTools operations that are not wrapped by a
high-level command:

```bash
evalraw <target> "DOM.getDocument" "{}"
evalraw <target> "Network.enable" "{\"maxPostDataSize\":10485760}"
```

Prefer the high-level `netclear`, `net`, and `netget` commands for normal
request debugging because they keep a per-tab event history.

## Browser and Profile Selection

Environment variables for the gateway process:

- `CDP_BROWSER=chrome|edge|brave|chromium`
- `CDP_BROWSER_MODE=launch|attach`
- `CDP_USER_DATA_DIR=<absolute path>`
- `CDP_PROFILE_MODE=persistent|clone|empty`
- `CDP_PROFILE_SOURCE_DIR=<absolute path>` for clone mode
- `CDP_RUNTIME_DIR=<absolute path>`

Default bundled mode is `launch` + `persistent`. Attach mode connects to an
already-running browser with remote debugging enabled and may trigger Chrome's
manual debugging approval prompt.

## Lifecycle

```bash
stop [target]
```

`stop` without a target stops per-tab CDP daemons, closes the managed browser,
and clears active runtime state such as `managed-browser.json`, `pages.json`,
and the active target cache. The managed browser profile remains on disk so
future launches reuse the same login/cache state.

`stop <target>` only stops that target's daemon and leaves the managed browser
running.

If a previous managed browser crashed or the gateway was interrupted, the next
command automatically clears stale runtime state and retries the managed browser
connection once.
