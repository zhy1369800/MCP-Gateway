const CODEGRAPH_NPM_PACKAGE: &str = "@colbymchenry/codegraph";

fn codegraph_tool_definition(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Value {
    json!({
            "name": BuiltinTool::CodeGraph.name(),
            "description": render_builtin_tool_description(BuiltinTool::CodeGraph, os, now, cfg.task_planning, cfg.read_file),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["exec"],
                "properties": {
                    "exec": {
                        "type": "string",
                        "description": "CodeGraph command to execute. First call must read the complete builtin://codegraph/SKILL.md. After reading it, use commands like 'codegraph status', 'codegraph init -i', 'codegraph query AuthService', or 'codegraph context \"task\"'."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Concrete project directory for CodeGraph. It must be inside one configured allowed directory. Required when more than one allowed directory exists."
                    },
                    "timeoutMs": {
                        "type": "integer",
                        "minimum": 1000,
                        "description": "Optional command timeout in milliseconds."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for every non-documentation call. First read the complete builtin://codegraph/SKILL.md without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Calls without the correct token fail and must be retried."
                    }
                }
            }
    })
}

fn codegraph_npx_program() -> &'static str {
    if cfg!(target_os = "windows") {
        "npx.cmd"
    } else {
        "npx"
    }
}

fn check_npx_available() -> bool {
    std::process::Command::new(codegraph_npx_program())
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
        .and_then(|mut child| {
            let start = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => return Some(status.success()),
                    Ok(None) if start.elapsed() < std::time::Duration::from_secs(5) => {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    _ => {
                        let _ = child.kill();
                        return None;
                    }
                }
            }
        })
        .unwrap_or(false)
}

fn codegraph_npx_args_from_exec(exec: &str) -> Result<Vec<String>, AppError> {
    let tokens = split_shell_tokens(exec);
    let Some((program, command_args)) = tokens.split_first() else {
        return Err(AppError::BadRequest("exec cannot be empty".to_string()));
    };

    if command_program_stem(program) != "codegraph" {
        return Err(AppError::BadRequest(format!(
            "codegraph tool only accepts commands starting with 'codegraph', got: {program}"
        )));
    }

    let Some(subcommand) = command_args.first() else {
        return Err(AppError::BadRequest(
            "codegraph command requires a subcommand such as status, init, query, files, context, affected, help, or --version".to_string(),
        ));
    };
    let normalized_subcommand = subcommand.to_ascii_lowercase();
    if matches!(
        normalized_subcommand.as_str(),
        "serve" | "install" | "uninit"
    ) {
        return Err(AppError::BadRequest(format!(
            "codegraph {subcommand} is not allowed in the built-in codegraph tool"
        )));
    }
    if !matches!(
        normalized_subcommand.as_str(),
        "init" | "index" | "sync" | "status" | "query" | "files" | "context" | "affected" | "help" | "--version"
    ) {
        return Err(AppError::BadRequest(format!(
            "codegraph subcommand is not allowed: {subcommand}"
        )));
    }

    let mut npx_args = vec![
        "-y".to_string(),
        CODEGRAPH_NPM_PACKAGE.to_string(),
    ];
    npx_args.extend(command_args.iter().cloned());
    Ok(npx_args)
}

fn command_program_stem(program: &str) -> String {
    let normalized = program.trim_matches('"').trim_matches('\'').to_ascii_lowercase();
    std::path::Path::new(&normalized)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or(normalized)
}

fn command_invokes_codegraph(program: &str, command_args: &[String]) -> bool {
    let stem = command_program_stem(program);
    if stem == "codegraph" {
        return true;
    }
    if stem != "npx" {
        return false;
    }
    command_args.iter().any(|arg| {
        let normalized = strip_matching_quotes(arg)
            .trim()
            .trim_end_matches(';')
            .to_ascii_lowercase();
        normalized == CODEGRAPH_NPM_PACKAGE
            || normalized.starts_with(&format!("{CODEGRAPH_NPM_PACKAGE}@"))
    })
}

impl SkillsService {
    async fn handle_builtin_codegraph(
        &self,
        config: &GatewayConfig,
        args: BuiltinShellArgs,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
        let call_id = Uuid::new_v4().to_string();
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
            BuiltinTool::CodeGraph.name(),
            &builtin_skill_token(BuiltinTool::CodeGraph),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        if let Some(result) = self
            .check_planning_gate(
                config,
                planning_scope,
                BuiltinTool::CodeGraph,
                args.planning_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        let cwd = match resolve_builtin_cwd(
            BuiltinTool::CodeGraph,
            &config.skills,
            args.cwd.as_deref(),
        ) {
            Ok(cwd) => cwd,
            Err(result) => return Ok(result),
        };

        let command_args = codegraph_npx_args_from_exec(&command_preview)?;
        let program = codegraph_npx_program();

        if !check_npx_available() {
            return Err(AppError::BadRequest(
                "npx is not available; install Node.js or npm first".to_string(),
            ));
        }

        let policy = evaluate_policy(
            &config.skills,
            program,
            &command_args,
            &command_preview,
            &cwd,
            None,
        );
        match policy {
            PolicyDecision::Deny(reason) => {
                return Ok(tool_error(
                    mcp_gateway_policy_denied_text(&reason),
                    json!({
                        "status": "blocked",
                        "reason": reason,
                        "tool": BuiltinTool::CodeGraph.name(),
                        "command": command_preview,
                        "actualCommand": std::iter::once(program.to_string()).chain(command_args.iter().cloned()).collect::<Vec<_>>().join(" "),
                        "cwd": normalize_display_path(&cwd),
                        "policyAction": "deny",
                        "policyHelp": mcp_gateway_policy_denied_help(&reason)
                    }),
                ));
            }
            PolicyDecision::Confirm { reason, reason_key } => {
                let display_tokens = split_shell_tokens(&command_preview);
                let metadata = ConfirmationMetadata {
                    kind: "shell".to_string(),
                    cwd: normalize_display_path(&cwd),
                    affected_paths: Vec::new(),
                    preview: command_preview.clone(),
                    reason_key,
                };
                let confirmation_id = match self
                    .create_confirmation_with_metadata(
                        "builtin:codegraph",
                        "CodeGraph",
                        &display_tokens,
                        &command_preview,
                        &reason,
                        metadata,
                    )
                    .await
                {
                    CreateConfirmationResult::Created(c) | CreateConfirmationResult::Reused(c) => {
                        c.id
                    }
                    CreateConfirmationResult::AlreadyTimedOut(id) => id,
                };

                match self
                    .wait_for_confirmation_decision(
                        &confirmation_id,
                        Self::CONFIRMATION_DECISION_TIMEOUT,
                        Duration::from_millis(250),
                    )
                    .await
                {
                    ConfirmationWaitOutcome::Approved => {}
                    ConfirmationWaitOutcome::Rejected => {
                        return Ok(confirmation_rejected_result(
                            BuiltinTool::CodeGraph.name(),
                            &confirmation_id,
                            false,
                        ));
                    }
                    ConfirmationWaitOutcome::TimedOut => {
                        return Ok(confirmation_rejected_result(
                            BuiltinTool::CodeGraph.name(),
                            &confirmation_id,
                            true,
                        ));
                    }
                }
            }
            PolicyDecision::Allow => {}
        }

        let timeout_ms = args
            .timeout_ms
            .unwrap_or(config.skills.execution.timeout_ms)
            .max(1000);
        let max_output_bytes = config.skills.execution.max_output_bytes.max(1024);
        self.record_tool_event_data(
            &call_id,
            BuiltinTool::CodeGraph.name(),
            "started",
            SkillToolEventData {
                cwd: Some(normalize_display_path(&cwd)),
                preview: Some(command_preview.clone()),
                ..SkillToolEventData::default()
            },
        )
        .await;

        let started = Instant::now();
        let mut command = Command::new(program);
        command
            .args(&command_args)
            .current_dir(&cwd)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        configure_bundled_tool_path(&mut command);
        configure_skill_command(&mut command);

        let output = match execute_skill_command(
            &mut command,
            timeout_ms,
            max_output_bytes,
            false,
            Some(SkillStreamEmitter {
                service: self.clone(),
                call_id: call_id.clone(),
                tool: BuiltinTool::CodeGraph.name().to_string(),
                kind: "stdoutDelta",
            }),
            Some(SkillStreamEmitter {
                service: self.clone(),
                call_id: call_id.clone(),
                tool: BuiltinTool::CodeGraph.name().to_string(),
                kind: "stderrDelta",
            }),
        )
        .await
        {
            Ok(output) => output,
            Err(error) => return Err(error),
        };
        let duration_ms = started.elapsed().as_millis() as u64;
        let stdout = output.stdout.text;
        let stderr = output.stderr.text;
        let exit_code = output.status.as_ref().and_then(|s| s.code()).unwrap_or(-1);

        if output.timed_out {
            self.record_tool_event_data(
                &call_id,
                BuiltinTool::CodeGraph.name(),
                "finished",
                SkillToolEventData {
                    status: Some("timed_out".to_string()),
                    exit_code: Some(exit_code),
                    duration_ms: Some(duration_ms),
                    ..SkillToolEventData::default()
                },
            )
            .await;
            let timeout_text = command_timeout_text(timeout_ms, &stdout, &stderr);
            return Ok(tool_error(
                timeout_text,
                json!({
                    "status": "timed_out",
                    "tool": BuiltinTool::CodeGraph.name(),
                    "command": command_preview,
                    "actualCommand": std::iter::once(program.to_string()).chain(command_args.iter().cloned()).collect::<Vec<_>>().join(" "),
                    "cwd": normalize_display_path(&cwd),
                    "exitCode": exit_code,
                    "durationMs": duration_ms,
                    "stdoutTruncated": output.stdout.truncated,
                    "stderrTruncated": output.stderr.truncated,
                    "timeoutMs": timeout_ms
                }),
            ));
        }

        let status = output.status.as_ref().expect("status must be Some when not timed out");
        let structured = json!({
            "status": if status.success() { "completed" } else { "failed" },
            "tool": BuiltinTool::CodeGraph.name(),
            "command": command_preview,
            "actualCommand": std::iter::once(program.to_string()).chain(command_args.iter().cloned()).collect::<Vec<_>>().join(" "),
            "cwd": normalize_display_path(&cwd),
            "exitCode": exit_code,
            "durationMs": duration_ms,
            "stdoutTruncated": output.stdout.truncated,
            "stderrTruncated": output.stderr.truncated
        });
        self.record_tool_event_data(
            &call_id,
            BuiltinTool::CodeGraph.name(),
            "finished",
            SkillToolEventData {
                status: Some(if status.success() {
                    "completed".to_string()
                } else {
                    "failed".to_string()
                }),
                exit_code: Some(exit_code),
                duration_ms: Some(duration_ms),
                ..SkillToolEventData::default()
            },
        )
        .await;
        let output_text = command_output_text(&stdout, &stderr);

        if status.success() {
            Ok(tool_success_with_planning_reminder(
                output_text,
                structured,
                self.planning_success_hints(
                    config,
                    planning_scope,
                    args.planning_id.as_deref(),
                    BuiltinTool::CodeGraph,
                    Some(&command_preview),
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
