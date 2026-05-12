---
name: shell_command
description: >-
  Run non-interactive shell commands inside an allowed workspace. First read the
  complete builtin://shell_command/SKILL.md to get skillToken; this SKILL.md
  read does not require skillToken. Do not use regex or partial reads to fetch
  only the token. Later calls without the correct skillToken will fail and must
  be retried, so get the token before running real commands. For search,
  discovery, broad inventories, and file counts, prefer rg or rg --files before
  recursive shell listings, and exclude dependency folders, VCS metadata,
  generated outputs, virtual environments, and caches such as node_modules,
  .git, target, dist, build, coverage, .next, .cache, .venv, and __pycache__.
metadata:
  bundled: true
  tool: shell_command
  category: terminal
---

# Shell Command

Use this bundled skill when the task is best handled by a terminal command: listing files, searching text, reading files, checking Git state, running builds/tests/formatters, starting a local dev server, or executing a project script.

This skill is intentionally a small terminal orchestration layer. It follows the same practical pattern used by coding agents such as Codex: keep commands non-interactive, keep the working directory explicit, prefer fast search/read tools, stream output back to the model, and let policy decide whether sensitive commands are allowed, denied, or require user confirmation.

## Global Search And Discovery Priority

- Treat `rg` as the default first tool for all project exploration and narrowing work, including project structure, file discovery, code navigation, symbol lookup, call-site discovery, config lookup, workflow tracing, tests, routes, error messages, broad inventories, and "where is this implemented?" questions.
- Use `rg` to locate candidate files and line numbers before reading file contents. Do not read many whole files to discover where something lives.
- Use `rg --files` first for project structure, file discovery, broad inventories, and file counts.
- Use `rg -n "pattern" path` first for text/code search, workflow tracing, symbol references, config keys, routes, CLI commands, and error strings.
- Use `rg -n -m 20 "pattern" path` when the initial result could be large.
- Use `rg --files | rg "name-or-extension"` for fast filename narrowing.
- Use `git ls-files` for "tracked files only" counts or inventories.
- Use `Get-ChildItem` only for simple, shallow Windows directory listings, for example `Get-ChildItem -Name` or `Get-ChildItem -Name -Filter *.rs`.
- Do not use `Get-ChildItem -Recurse`, `find . -type f`, `dir /s`, `ls -R`, or `du` for repository-wide discovery, search, inventories, or counts unless the command includes explicit exclusions and `rg` is unavailable.
- Never treat dependency folders, VCS metadata, generated outputs, virtual environments, or caches as meaningful project files in broad counts.

If `rg` is not found, use `git ls-files` for tracked-file discovery before falling back to platform recursive listings. When falling back to recursive listings, include explicit exclusions for dependency folders, VCS metadata, generated outputs, virtual environments, and caches.

## Project And Workflow Navigation With Ripgrep

Use `rg` as the main project navigation tool, not just as a project-structure tool and not only as a code-search tool. The normal investigation loop is:

1. Start with `rg --files` when you need the project shape, likely directories, file names, or extension distribution.
2. Search names, strings, routes, config keys, error text, workflow steps, CLI commands, tests, or public API shapes with `rg -n`.
3. Narrow by directory, extension, or file name when the result is broad.
4. Read only the few matching files or line ranges after the likely location is known.
5. Repeat with nearby identifiers from those results until the implementation path is clear.

Prefer this pattern over opening many files and scanning them manually. Reading file contents is for understanding confirmed candidates; `rg` is for finding those candidates.

Useful patterns:

- Find symbol references: `rg -n "SkillConfirmation|handle_builtin_shell_command" crates`
- Find routes, commands, or config keys: `rg -n "tools/call|skillToken|allowedDirs" .`
- Find likely files first, then search inside them: `rg --files | rg "skill|tool|config"`
- Limit noisy searches: `rg -n -m 20 "pattern" path`
- Search specific file types: `rg -n "pattern" -g "*.rs" -g "*.ts" .`

## Operating Model

- Always set `cwd` to the concrete directory where the command should run.
- `cwd` must be inside one configured allowed directory.
- If more than one allowed directory is configured and the user did not specify the target workspace, ask which directory should be used before running a command.
- Use one command string in `exec`; the gateway runs it through the platform shell. Do not include a shell executable prefix unless you intentionally need a nested shell.
- On Windows, commands are run through `powershell -NoProfile -ExecutionPolicy Bypass -Command ...` with UTF-8 handling. Write the `exec` string as PowerShell code by default; do not prefix normal commands with `powershell`.
- On Unix-like systems, commands are run through `sh -lc ...`. Write the `exec` string as POSIX shell code by default.
- Commands are non-interactive. Do not launch editors, pagers, REPLs, curses/full-screen UIs, or prompts that wait for input.
- Use `timeoutMs` when a command may take longer than the configured default. Keep it bounded.
- Treat shell commands as the wrong default for manual file edits. When the task is to create, edit, delete, move, or rewrite source/config/docs files by hand, prefer the bundled `multi_edit_file` skill first.

## Windows PowerShell Style

On Windows, treat PowerShell as the primary shell. The gateway already launches the `exec` text through PowerShell and configures UTF-8 input/output before running the requested command. This means `exec` should usually be `Get-ChildItem -Name`, not `powershell Get-ChildItem -Name`. The UTF-8 handling covers terminal text encoding; it does not make `cmd.exe`-specific syntax valid inside PowerShell.

External programs are still called directly from PowerShell:

- `git status --short`
- `cargo test`
- `npm run lint`
- `rg -n "pattern" src`

Prefer native PowerShell cmdlets and parameters for shallow filesystem operations. For codebase navigation, search, discovery, feature tracing, broad inventories, and counts, use `rg` first.

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
- Search with line numbers: `rg -n "functionName|StructName|configKey" src`
- Find definitions and call sites before reading files: `rg -n "handleFoo|FooConfig|foo_enabled" src`
- Locate likely files by name: `rg --files | rg "skills|gateway|config"`
- Count project files: `(rg --files <exclusions> | Measure-Object).Count`
- List files on Windows: `Get-ChildItem -Name -Filter *.js`
- Read files on Windows: `Get-Content -LiteralPath path -TotalCount 200`
- Read files on Unix-like systems: `sed -n '1,200p' path`
- Inspect Git: `git status --short`, `git diff -- path`, `git log --oneline -n 20`
- Run project checks: `cargo test`, `cargo fmt --check`, `npm test`, `npm run lint`
- Start a project-owned dev server only when the user needs a running app and the server is expected to stay alive.

Prefer `rg` and `rg --files` for discovery because they are fast and respect ignore files. Packaged gateway builds may provide a bundled `rg` on `PATH`; if `rg` is unavailable, use `git ls-files` for repository counts or the platform's normal alternative with explicit exclusions.

## Counting Files And Broad Inventories

When the user asks for file counts, repository size, broad file inventories, or similar numeric summaries, do not count dependency folders, VCS metadata, generated outputs, virtual environments, or caches as meaningful project files. These directories can dominate the result and make the answer misleading. This is a hard requirement, not a preference.

Use commands that respect `.gitignore` and exclude high-volume irrelevant directories before counting:

- Tracked files only: `git ls-files | Measure-Object` on Windows, or `git ls-files | wc -l` on Unix-like systems.
- All non-ignored files with ripgrep on Windows: `(rg --files -g '!node_modules/**' -g '!.git/**' -g '!target/**' -g '!dist/**' -g '!build/**' -g '!coverage/**' -g '!.next/**' -g '!.nuxt/**' -g '!.turbo/**' -g '!.cache/**' -g '!__pycache__/**' -g '!.pytest_cache/**' -g '!.mypy_cache/**' -g '!.ruff_cache/**' -g '!.venv/**' -g '!venv/**' -g '!env/**' -g '!.tox/**' | Measure-Object).Count`
- All non-ignored files with ripgrep on Unix-like systems: `rg --files -g '!node_modules/**' -g '!.git/**' -g '!target/**' -g '!dist/**' -g '!build/**' -g '!coverage/**' -g '!.next/**' -g '!.nuxt/**' -g '!.turbo/**' -g '!.cache/**' -g '!__pycache__/**' -g '!.pytest_cache/**' -g '!.mypy_cache/**' -g '!.ruff_cache/**' -g '!.venv/**' -g '!venv/**' -g '!env/**' -g '!.tox/**' | wc -l`

Common directories to exclude from broad counts and inventories include:

- Version control metadata: `.git`, `.svn`, `.hg`
- JavaScript and frontend dependencies/caches: `node_modules`, `.pnpm-store`, `.yarn`, `.next`, `.nuxt`, `.turbo`, `.parcel-cache`, `bower_components`
- Build and coverage output: `target`, `dist`, `build`, `out`, `coverage`, `.coverage`, `htmlcov`
- Python environments and caches: `.venv`, `venv`, `env`, `.tox`, `__pycache__`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.nox`
- General tool caches: `.cache`, `.gradle`, `.idea`, `.vscode`, `tmp`, `temp`

Do not use broad recursive commands such as `Get-ChildItem -Recurse`, `find . -type f`, `dir /s`, `ls -R`, or `du` at the repository root for counts or inventories unless they include explicit exclusions or the workspace is already known to be small. If `rg` is unavailable, prefer `git ls-files` for repository counts. For untracked non-ignored files without `rg`, use the platform's recursive listing only with exclusion filters.

## File Reading

Use the bundled `read_file` skill as the default way to read text files. It returns stable line-numbered output, enforces allowed-directory boundaries before reading, rejects binary files, and limits large reads. Use terminal reads only when `read_file` is disabled, unavailable, or clearly unsuitable for the target file type.

Use terminal commands for directory listings, search, generated command output, test/build output, and project-owned tools. Do not use `cat`, `Get-Content`, `type`, `sed`, `head`, or `tail` for ordinary source/config file reads when `read_file` can do the job.

## File Editing

Use bundled editing skills as the default way to create, modify, or rewrite files by hand. Use `multi_edit_file` for exact replacements, multi-file edits, file creation, deletion, and moves.

Prefer these editing skills because they give the gateway a clear set of affected paths before files are changed, produce reviewable deltas, and avoid fragile shell redirection or quoting problems.

Use shell-based writes only as a fallback after the bundled editing skills are clearly unsuitable or have failed repeatedly, or when an external tool is the correct owner of the output. Valid examples include:

- Running a formatter that rewrites files.
- Running a code generator, scaffold command, package manager, or project script.
- Producing binary files or very large mechanical outputs that cannot reasonably be expressed as structured file operations.
- Applying an upstream patch file with `git apply` or a dedicated patch tool.
- Performing a large, repetitive, structure-preserving migration that is safer as a small script than as a huge hand-written edit request.

Avoid ad hoc terminal writes such as `echo ... > file`, here-documents, `Set-Content`, `Out-File`, or scripts that manually rewrite source files when `multi_edit_file` can express the change. If shell-based writing is used as a fallback, explain why the bundled editing skill was not sufficient and keep the command narrowly scoped.

For batch edits to structured files, use structured APIs where practical. Use a JSON parser for JSON instead of regex. For code files, prefer narrow block-aware transformations, generated code followed by the project formatter, or project-owned tooling. Avoid broad regex replacements over Rust, JSON, YAML, TOML, or source files unless the pattern is tightly scoped, verified against current content, and followed by targeted validation.

After any shell-based write, inspect the resulting changes before reporting success:

- Use `git diff -- path` for focused edits.
- Use `git diff --stat` for multi-file edits.
- Run the smallest meaningful formatter, parser, build, or test command for the touched files.
- If the diff includes unrelated changes, stop and report the discrepancy instead of continuing blindly.

## Coding Change Discipline

When the terminal is being used to investigate or change code, do not treat "the patch applied" as the finish line. The reliable workflow is to understand the system's intent, make the smallest useful change, and verify the behavior that motivated the change.

- Reconstruct the behavior path before changing code. Identify the inputs, transformations, persistence boundary, output, and any tests or callers that define the contract.
- Prefer evidence from the current codebase over naming guesses. Use search to locate definitions, call sites, adapters, serializers, renderers, configuration, and tests before deciding where the problem lives.
- Separate model, transport, storage, and presentation concerns. A correct fix usually preserves the layer contract rather than converting one layer's representation into another's.
- Preserve stable identifiers, metadata, schema fields, ordering, user-authored values, and compatibility behavior unless the requested change specifically requires altering them.
- Keep edits scoped to the smallest surface that closes the behavior gap. Avoid broad rewrites, new abstractions, or configuration churn when a local contract fix is enough.
- Before editing, check `git status --short` and inspect files you will touch. Work with existing user changes and do not revert unrelated modifications.
- When changing conversion, normalization, form, parser, serializer, or migration code, explicitly check round-trip behavior and optional fields.
- After editing, inspect the focused diff and confirm that the diff expresses the intended behavior. A passing command does not replace reading the diff.
- Run the smallest meaningful verification command for the touched area: targeted unit tests, typecheck, parser check, formatter check, or build. If no verification is practical, report that and state the residual risk.
- For bug fixes, add or update a focused test when the project already has a nearby test setup and the behavior is easy to express.
- If validation reveals a new issue, continue the loop until the change is correct or a real blocker is found. Do not report completion after only the first edit.
- Be decisive when evidence is sufficient. Ask the user only when required context cannot be inferred safely, when multiple product behaviors are plausible, or when the next action is destructive or externally risky.

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
- For normal file reads, prefer `read_file` with `offset` and `limit`. Start around 120-200 lines when inspecting a known region rather than reading the whole file.
- On Windows, prefer `Get-Content -LiteralPath path -TotalCount 200` for the start of a file, or `Get-Content -LiteralPath path | Select-Object -Skip 200 -First 200` for the next chunk.
- On Unix-like systems, prefer `sed -n '1,200p' path`, then `sed -n '201,400p' path` if needed.
- Avoid `Get-Content -Raw`, `cat large-file`, broad `git diff`, recursive directory listings, and commands with unbounded logs unless the file or output is already known to be small.
- For search, narrow by path and use limits when possible: `rg -n "pattern" path`, `rg -n -m 20 "pattern" path`, or `rg --files path`.
- For Git inspection, prefer scoped and bounded commands such as `git diff -- path`, `git diff --stat`, `git log --oneline -n 20`, and `git show --stat`.
- When command output is still too broad, rerun with a narrower path, pattern, line range, or summary flag instead of asking for all output.

If a command fails, use the exit code and stderr/stdout to decide the next step. Do not blindly rerun a failing command with broader scope.

## Recommended Workflow

1. Establish the workspace with `cwd`.
2. Inspect before changing: `git status --short`, then use `rg`/`rg --files` to locate relevant files, symbols, call sites, config keys, and line numbers.
3. Use `read_file` to read only the targeted files or line ranges that `rg` identified. Avoid reading broad file contents as a discovery strategy.
4. Use the narrowest command that answers the question.
5. Prefer `multi_edit_file` for manual source edits.
6. For large mechanical migrations, use a narrowly scoped script only after reading the current target content; prefer structured parsers over regex.
7. Inspect the relevant diff with `git diff -- path` or `git diff --stat`.
8. Run the smallest meaningful verification command after changes.
9. Report what changed and what verification passed or could not be run.
