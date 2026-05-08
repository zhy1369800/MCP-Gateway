---
name: shell_command
description: Run non-interactive shell commands inside an allowed workspace for inspection, builds, tests, and narrowly scoped automation.
metadata:
  bundled: true
  tool: shell_command
  category: terminal
---

# Shell Command

Use this bundled skill when the task is best handled by a terminal command: listing files, searching text, reading files, checking Git state, running builds/tests/formatters, starting a local dev server, or executing a project script.

This skill is intentionally a small terminal orchestration layer. It follows the same practical pattern used by coding agents such as Codex: keep commands non-interactive, keep the working directory explicit, prefer fast search/read tools, stream output back to the model, and let policy decide whether sensitive commands are allowed, denied, or require user confirmation.

## Operating Model

- Always set `cwd` to the concrete directory where the command should run.
- `cwd` must be inside one configured allowed directory.
- If more than one allowed directory is configured and the user did not specify the target workspace, ask which directory should be used before running a command.
- Use one command string in `exec`; the gateway runs it through the platform shell. Do not include a shell executable prefix unless you intentionally need a nested shell.
- On Windows, commands are run through `powershell -NoProfile -ExecutionPolicy Bypass -Command ...` with UTF-8 handling. Write the `exec` string as PowerShell code by default; do not prefix normal commands with `powershell`.
- On Unix-like systems, commands are run through `sh -lc ...`. Write the `exec` string as POSIX shell code by default.
- Commands are non-interactive. Do not launch editors, pagers, REPLs, curses/full-screen UIs, or prompts that wait for input.
- Use `timeoutMs` when a command may take longer than the configured default. Keep it bounded.

## Windows PowerShell Style

On Windows, treat PowerShell as the primary shell. The gateway already launches the `exec` text through PowerShell and configures UTF-8 input/output before running the requested command. This means `exec` should usually be `Get-ChildItem -Name`, not `powershell Get-ChildItem -Name`. The UTF-8 handling covers terminal text encoding; it does not make `cmd.exe`-specific syntax valid inside PowerShell.

External programs are still called directly from PowerShell:

- `git status --short`
- `cargo test`
- `npm run lint`
- `rg -n "pattern" src`

Prefer native PowerShell cmdlets and parameters:

- List files: `Get-ChildItem -Name`, `Get-ChildItem -Name -Filter *.js`
- List a specific directory: `Get-ChildItem -LiteralPath "D:\path with spaces" -Name`
- Read a file: `Get-Content -LiteralPath file.txt -TotalCount 200`
- Test a path: `Test-Path -LiteralPath "D:\path"`
- Remove a file only when clearly requested and allowed: `Remove-Item -LiteralPath file.txt`
- Set an environment variable for later commands in the same `exec`: `$env:NAME = 'value'; npm test`
- Run a command only if the previous external command succeeded: `cargo fmt --check; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; cargo test`

Assume Windows PowerShell compatibility unless the environment explicitly says `pwsh` is used. Avoid PowerShell 7-only operators such as `&&` and `||` for control flow in Windows examples.

Avoid `cmd.exe` built-ins and switches unless you explicitly invoke `cmd.exe` as a fallback. In PowerShell, commands such as `dir /b`, `copy`, `del`, `type`, `set NAME=value`, and `%VAR%` expansion do not behave like they do in `cmd.exe`. For example, `dir /b` is wrong in PowerShell because `/b` is parsed as a path; use `Get-ChildItem -Name` instead.

Use `cmd.exe` only for compatibility with a command that genuinely requires `cmd.exe` syntax:

- `cmd.exe /d /c dir /b *.js`
- `cmd.exe /d /c some-legacy-script.bat`

Keep the `cwd` argument set to the target workspace even when using `cmd.exe` as a fallback. Do not switch directories inside the command unless a project script requires it.

## Unix-Like Shell Style

On Unix-like systems, the gateway runs `exec` through `sh -lc`. Prefer POSIX-compatible shell syntax in examples unless the task explicitly requires Bash, Zsh, Fish, or another shell.

- List files: `ls -1`
- Read a file chunk: `sed -n '1,200p' file.txt`
- Set an environment variable for one command: `NAME=value npm test`
- Chain commands on success: `cargo fmt --check && cargo test`
- Invoke Bash only when needed: `bash -lc 'source scripts/env.sh && ./scripts/test.sh'`

Use forward-slash paths and quote paths with spaces. Keep `cwd` set to the workspace instead of relying on `cd` where possible.

## Good Uses

- Discover files: `rg --files`
- Search code/text: `rg "pattern" path`
- List files on Windows: `Get-ChildItem -Name -Filter *.js`
- Read files on Windows: `Get-Content -LiteralPath path -TotalCount 200`
- Read files on Unix-like systems: `sed -n '1,200p' path`
- Inspect Git: `git status --short`, `git diff -- path`, `git log --oneline -n 20`
- Run project checks: `cargo test`, `cargo fmt --check`, `npm test`, `npm run lint`
- Start a project-owned dev server only when the user needs a running app and the server is expected to stay alive.

Prefer `rg` and `rg --files` for discovery because they are fast and ignore common generated directories by default. If `rg` is unavailable, use the platform's normal alternative.

## File Editing

Use the bundled `apply_patch` skill for structured file edits when possible. It gives the gateway a clear set of affected paths before files are changed and avoids fragile shell redirection.

Use shell-based writes only when a formatter, generator, package manager, or project script is the right owner of the output. Avoid ad hoc redirection such as `echo ... > file` for source edits.

## Safety And Policy

The gateway evaluates commands against the configured skill policy.

- A command can be allowed immediately.
- A command can be denied with a policy reason.
- A command can require user confirmation.
- The path guard can block commands that try to operate outside allowed directories.

Treat destructive commands as high risk: recursive deletes, force moves, permission changes, process kills, package publishing, credential operations, and networked deployment commands should only run when the user clearly asked for them and policy allows them.

On Windows, do not compose destructive file operations by piping paths into another shell. Prefer native PowerShell cmdlets end to end, use `-LiteralPath`, and verify resolved absolute paths before recursive delete or move operations.

## Output Handling

The gateway captures stdout and stderr and returns them to the caller. Markdown file reads are allowed to return larger output so a model can progressively load `SKILL.md` files and other instructions. Other command output may be truncated according to the configured limit.

For real command execution, the gateway also records lightweight tool events that admin clients can poll at `/api/v2/admin/skills/events?after=<seq>`. Shell commands emit `started`, `stdoutDelta`, `stderrDelta`, and `finished` events. These events are for UI/progress display; the final tool result remains the authoritative command result.

Keep terminal output intentionally small. Terminal commands can produce unexpectedly large output, and sending that output back to the model wastes context and tokens.

- Prefer reading in bounded chunks, then continue only if more context is needed.
- For normal file reads, start around 120-200 lines rather than reading the whole file.
- On Windows, prefer `Get-Content -LiteralPath path -TotalCount 200` for the start of a file, or `Get-Content -LiteralPath path | Select-Object -Skip 200 -First 200` for the next chunk.
- On Unix-like systems, prefer `sed -n '1,200p' path`, then `sed -n '201,400p' path` if needed.
- Avoid `Get-Content -Raw`, `cat large-file`, broad `git diff`, recursive directory listings, and commands with unbounded logs unless the file or output is already known to be small.
- For search, narrow by path and use limits when possible: `rg -n "pattern" path`, `rg -n -m 20 "pattern" path`, or `rg --files path`.
- For Git inspection, prefer scoped and bounded commands such as `git diff -- path`, `git diff --stat`, `git log --oneline -n 20`, and `git show --stat`.
- When command output is still too broad, rerun with a narrower path, pattern, line range, or summary flag instead of asking for all output.

If a command fails, use the exit code and stderr/stdout to decide the next step. Do not blindly rerun a failing command with broader scope.

## Recommended Workflow

1. Establish the workspace with `cwd`.
2. Inspect before changing: `git status --short`, `rg --files`, targeted file reads.
3. Use the narrowest command that answers the question.
4. Prefer `apply_patch` for manual source edits.
5. Run the smallest meaningful verification command after changes.
6. Report what changed and what verification passed or could not be run.
