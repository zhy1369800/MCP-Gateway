fn chrome_cdp_tool_definition(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Value {
    json!({
            "name": BuiltinTool::ChromeCdp.name(),
            "description": render_builtin_tool_description(BuiltinTool::ChromeCdp, os, now, cfg.task_planning, cfg.read_file),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["exec"],
                "properties": {
                    "exec": {
                        "type": "string",
                        "description": "Chrome DevTools Protocol command. First call must read the complete builtin://chrome-cdp/SKILL.md. After reading it, use commands like `open <url>`, `list`, `snap`, `eval`, `netclear`, `net <filter>`, `netget <id>`, or `click <target> <selector>`."
                    },
                    "timeoutMs": {
                        "type": "integer",
                        "minimum": 1000,
                        "description": "Optional command timeout in milliseconds."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for every non-documentation call. First read the complete builtin://chrome-cdp/SKILL.md without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Calls without the correct token fail and must be retried."
                    }
                }
            }
    })
}

impl SkillsService {
    async fn handle_builtin_chrome_cdp(
        &self,
        config: &GatewayConfig,
        args: BuiltinShellArgs,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
        let command_preview = args.exec.trim().to_string();
        if command_preview.is_empty() {
            return Err(AppError::BadRequest("exec cannot be empty".to_string()));
        }

        if let Some((tool, matched_path)) = builtin_skill_doc_read(&command_preview) {
            return Ok(builtin_skill_doc_result(
                tool,
                &command_preview,
                matched_path,
                builtin_skill_token(tool),
                Self::planning_enabled(config),
            ));
        }

        if let Some(result) = validate_skill_token_result(
            BuiltinTool::ChromeCdp.name(),
            &builtin_skill_token(BuiltinTool::ChromeCdp),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        if let Some(result) = self
            .check_planning_gate(
                config,
                planning_scope,
                BuiltinTool::ChromeCdp,
                args.planning_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        self.execute_builtin_chrome_cdp_command(
            config,
            BuiltinTool::ChromeCdp.name(),
            &command_preview,
            &command_preview,
            args.timeout_ms,
            planning_scope,
            args.planning_id.as_deref(),
            BuiltinTool::ChromeCdp,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_builtin_chrome_cdp_command(
        &self,
        config: &GatewayConfig,
        tool_name: &str,
        command_preview: &str,
        structured_command: &str,
        timeout_ms: Option<u64>,
        planning_scope: &str,
        planning_id: Option<&str>,
        planning_tool: BuiltinTool,
    ) -> Result<ToolResult, AppError> {
        let cdp_args = parse_builtin_chrome_cdp_args(command_preview)?;
        let cdp_script = materialize_builtin_chrome_cdp_script()?;
        let cdp_runtime_dir = builtin_chrome_cdp_runtime_dir()?;
        let cdp_user_data_dir = builtin_chrome_cdp_user_data_dir()?;
        let effective_user_data_dir = std::env::var_os("CDP_USER_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| cdp_user_data_dir.clone());
        let effective_profile_mode =
            std::env::var("CDP_PROFILE_MODE").unwrap_or_else(|_| "persistent".to_string());
        let effective_browser_mode =
            std::env::var("CDP_BROWSER_MODE").unwrap_or_else(|_| "launch".to_string());
        let timeout_ms = timeout_ms
            .unwrap_or_else(|| {
                config
                    .skills
                    .execution
                    .timeout_ms
                    .max(BUILTIN_CHROME_CDP_DEFAULT_TIMEOUT_MS)
            })
            .max(1000);
        let max_output_bytes = config.skills.execution.max_output_bytes.max(1024);

        let started = Instant::now();
        let mut command = Command::new(node_command());
        command
            .arg(&cdp_script)
            .args(&cdp_args)
            .env("CDP_RUNTIME_DIR", &cdp_runtime_dir)
            .env("CDP_USER_DATA_DIR", &effective_user_data_dir)
            .env("CDP_PROFILE_MODE", &effective_profile_mode)
            .env("CDP_BROWSER_MODE", &effective_browser_mode)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        configure_bundled_tool_path(&mut command);
        configure_skill_command(&mut command);

        let output = execute_skill_command(
            &mut command,
            timeout_ms,
            max_output_bytes,
            false,
            None,
            None,
        )
        .await?;
        let duration_ms = started.elapsed().as_millis() as u64;
        let stdout = output.stdout.text;
        let stderr = output.stderr.text;
        let exit_code = output.status.code().unwrap_or(-1);
        let structured_cdp_args = if tool_name == BuiltinTool::ChatPlusAdapterDebugger.name()
            && cdp_args.first().map(|arg| arg.as_str()) == Some("eval")
        {
            vec!["eval".to_string(), "[eval omitted]".to_string()]
        } else {
            cdp_args.clone()
        };

        let mut structured = json!({
            "status": if output.status.success() { "completed" } else { "failed" },
            "tool": tool_name,
            "command": structured_command,
            "runner": node_command(),
            "args": structured_cdp_args,
            "profileMode": effective_profile_mode,
            "browserMode": effective_browser_mode,
            "exitCode": exit_code,
            "durationMs": duration_ms,
            "stdoutTruncated": output.stdout.truncated,
            "stderrTruncated": output.stderr.truncated
        });
        if skill_debug_metadata_enabled() {
            if let Value::Object(fields) = &mut structured {
                fields.insert(
                    "script".to_string(),
                    Value::String(normalize_display_path(&cdp_script)),
                );
                fields.insert(
                    "runtimeDir".to_string(),
                    Value::String(normalize_display_path(&cdp_runtime_dir)),
                );
                fields.insert(
                    "userDataDir".to_string(),
                    Value::String(normalize_display_path(&effective_user_data_dir)),
                );
            }
        }
        let output_text = command_output_text(&stdout, &stderr);

        if output.status.success() {
            Ok(tool_success_with_planning_reminder(
                output_text,
                structured,
                self.planning_success_hints(
                    config,
                    planning_scope,
                    planning_id,
                    planning_tool,
                    None,
                )
                .await,
            ))
        } else {
            Ok(tool_error(
                command_failure_text(exit_code, &stdout, &stderr),
                structured,
            ))
        }
    }
}

fn materialize_builtin_chrome_cdp_script() -> Result<PathBuf, AppError> {
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("mcp-gateway")
        .join("builtin-skills")
        .join("chrome-cdp")
        .join("scripts");
    fs::create_dir_all(&dir)?;
    let path = dir.join("cdp.mjs");
    let should_write = match fs::read_to_string(&path) {
        Ok(existing) => existing != BUILTIN_CHROME_CDP_MJS,
        Err(_) => true,
    };
    if should_write {
        fs::write(&path, BUILTIN_CHROME_CDP_MJS)?;
    }
    Ok(path)
}


fn parse_builtin_chrome_cdp_args(command: &str) -> Result<Vec<String>, AppError> {
    let tokens = split_shell_tokens(command);
    let Some((program, args)) = tokens.split_first() else {
        return Err(AppError::BadRequest("exec cannot be empty".to_string()));
    };
    let normalized_program = normalize_command_token(program);

    if is_chrome_cdp_node_invocation(&normalized_program, args) {
        return Ok(args[1..].to_vec());
    }

    if is_chrome_cdp_script_token(program) || normalized_program == "cdp" {
        return Ok(args.to_vec());
    }

    if is_chrome_cdp_cli_command(&normalized_program) || is_chrome_cdp_cli_flag(&normalized_program)
    {
        return Ok(tokens);
    }

    Err(AppError::BadRequest(
        "chrome-cdp uses the bundled raw CDP runner. Command must be a documented CDP subcommand such as `open`, `list`, `snap`, `eval`, `netclear`, `net`, `netget`, `click`, or `stop` after SKILL.md has been read".to_string(),
    ))
}

fn cdp_command_from_parts_with_prefix(prefix: &str, parts: &[String]) -> Result<String, AppError> {
    let mut all = Vec::with_capacity(parts.len() + 1);
    all.push(prefix.to_string());
    all.extend(parts.iter().cloned());
    cdp_command_from_parts(&all)
}

fn cdp_command_from_parts(parts: &[String]) -> Result<String, AppError> {
    parts
        .iter()
        .map(|part| quote_cdp_command_part(part))
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join(" "))
}

fn quote_cdp_command_part(part: &str) -> Result<String, AppError> {
    if part.is_empty() {
        return Ok("\"\"".to_string());
    }
    if !part
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '\'' || ch == '"')
    {
        return Ok(part.to_string());
    }
    if !part.contains('"') {
        return Ok(format!("\"{part}\""));
    }
    if !part.contains('\'') {
        return Ok(format!("'{part}'"));
    }
    Err(AppError::BadRequest(
        "CDP command arguments cannot contain both single and double quotes".to_string(),
    ))
}

fn is_chrome_cdp_node_invocation(normalized_program: &str, args: &[String]) -> bool {
    matches!(normalized_program, "node" | "node.exe")
        && args
            .first()
            .is_some_and(|arg| is_chrome_cdp_script_token(arg))
}

fn is_chrome_cdp_script_token(token: &str) -> bool {
    normalize_command_token(token).ends_with("cdp.mjs")
}

fn is_chrome_cdp_cli_flag(command: &str) -> bool {
    matches!(command, "--help" | "-v" | "-V" | "--version" | "--full")
}

fn is_chrome_cdp_cli_command(command: &str) -> bool {
    matches!(
        command,
        "launch"
            | "open"
            | "list"
            | "ls"
            | "snap"
            | "snapshot"
            | "screenshot"
            | "shot"
            | "click"
            | "clickxy"
            | "type"
            | "eval"
            | "html"
            | "nav"
            | "navigate"
            | "net"
            | "network"
            | "netclear"
            | "network-clear"
            | "netget"
            | "network-get"
            | "perfnet"
            | "loadall"
            | "evalraw"
            | "stop"
            | "help"
    )
}

fn builtin_chrome_cdp_runtime_dir() -> Result<PathBuf, AppError> {
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("mcp-gateway")
        .join("builtin-skills")
        .join("chrome-cdp")
        .join("runtime");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn builtin_chrome_cdp_user_data_dir() -> Result<PathBuf, AppError> {
    let dir = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("mcp-gateway")
        .join("builtin-skills")
        .join("chrome-cdp")
        .join("chrome-user-data");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(target_os = "windows")]
fn node_command() -> &'static str {
    "node.exe"
}

#[cfg(not(target_os = "windows"))]
fn node_command() -> &'static str {
    "node"
}

