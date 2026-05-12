/// Check if officecli binary is callable, with a 5-second timeout.
/// If `path` is Some, use that exact binary; otherwise try "officecli" on PATH.
/// For directories, appends "officecli" or "officecli.exe" automatically.
pub(crate) fn check_officecli_command(path: Option<&str>) -> bool {
    let binary = resolve_officecli_binary(path);
    std::process::Command::new(&binary)
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

/// Resolve a user-supplied path (or default "officecli") to an actual binary path.
/// If the given path is a directory, appends the platform-appropriate binary name.
pub(crate) fn resolve_officecli_binary(raw_path: Option<&str>) -> String {
    let raw = raw_path.unwrap_or("officecli");
    let resolved = std::path::Path::new(raw);
    if resolved.is_dir() {
        let exe_name = if cfg!(target_os = "windows") { "officecli.exe" } else { "officecli" };
        resolved.join(exe_name).to_string_lossy().into_owned()
    } else {
        raw.to_string()
    }
}

/// Convenience wrapper: checks using the configured custom path if present, else PATH.
pub(crate) fn check_officecli_available(cfg: &BuiltinToolsConfig) -> bool {
    check_officecli_command(cfg.office_cli_path.as_deref())
}

/// Return the binary to use for execution: resolves directories and uses custom path if configured, else "officecli".
pub(crate) fn officecli_program(cfg: &BuiltinToolsConfig) -> String {
    resolve_officecli_binary(cfg.office_cli_path.as_deref())
}

fn office_cli_tool_definition(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Value {
    json!({
            "name": BuiltinTool::OfficeCli.name(),
            "description": render_builtin_tool_description(BuiltinTool::OfficeCli, os, now, cfg.task_planning, cfg.read_file),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["exec"],
                "properties": {
                    "exec": {
                        "type": "string",
                        "description": "officecli command to execute. First call must read the complete builtin://officecli/SKILL.md. After reading it, use commands like 'officecli create file.docx', 'officecli set ...', 'officecli get ...' etc."
                    },
                    "timeoutMs": {
                        "type": "integer",
                        "minimum": 1000,
                        "description": "Optional command timeout in milliseconds."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for every non-documentation call. First read the complete builtin://officecli/SKILL.md without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Calls without the correct token fail and must be retried."
                    }
                }
            }
    })
}

impl SkillsService {
    async fn handle_builtin_office_cli(
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
            BuiltinTool::OfficeCli.name(),
            &builtin_skill_token(BuiltinTool::OfficeCli),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        if let Some(result) = self
            .check_planning_gate(
                config,
                planning_scope,
                BuiltinTool::OfficeCli,
                args.planning_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        // Guard: verify officecli binary is callable
        if !check_officecli_available(&config.skills.builtin_tools) {
            return Err(AppError::BadRequest(
                "officecli is not available on this system".to_string(),
            ));
        }

        let cwd = match resolve_builtin_cwd(
            BuiltinTool::OfficeCli,
            &config.skills,
            args.cwd.as_deref(),
        ) {
            Ok(cwd) => cwd,
            Err(result) => return Ok(result),
        };

        let tokens = split_shell_tokens(&command_preview);
        if tokens.is_empty() {
            return Err(AppError::BadRequest("exec cannot be empty".to_string()));
        }
        let program = tokens[0].clone();
        // Restrict: must start with officecli or officecli.exe
        let allowed_program = officecli_program(&config.skills.builtin_tools);
        let allowed_name = std::path::Path::new(&allowed_program)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "officecli".to_string());
        if program != "officecli" && program != "officecli.exe" && program != allowed_name && program != allowed_program {
            return Err(AppError::BadRequest(format!(
                "officecli tool only accepts 'officecli' or 'officecli.exe' as program, got: {program}"
            )));
        }
        let command_args = tokens[1..].to_vec();

        let policy = evaluate_policy(
            &config.skills,
            &program,
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
                        "tool": BuiltinTool::OfficeCli.name(),
                        "command": command_preview,
                        "cwd": normalize_display_path(&cwd),
                        "policyAction": "deny",
                        "policyHelp": mcp_gateway_policy_denied_help(&reason)
                    }),
                ));
            }
            PolicyDecision::Confirm { reason, reason_key } => {
                let metadata = ConfirmationMetadata {
                    kind: "shell".to_string(),
                    cwd: normalize_display_path(&cwd),
                    affected_paths: Vec::new(),
                    preview: command_preview.clone(),
                    reason_key,
                };
                let confirmation_id = match self
                    .create_confirmation_with_metadata(
                        "builtin:officecli",
                        "OfficeCLI",
                        &tokens,
                        &command_preview,
                        &reason,
                        metadata,
                    )
                    .await
                {
                    CreateConfirmationResult::Created(c) | CreateConfirmationResult::Reused(c) => c.id,
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
                            BuiltinTool::OfficeCli.name(),
                            &confirmation_id,
                            false,
                        ));
                    }
                    ConfirmationWaitOutcome::TimedOut => {
                        return Ok(confirmation_rejected_result(
                            BuiltinTool::OfficeCli.name(),
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
        // Execute officecli directly, not through a shell wrapper.
        // The caller's first token was already validated to be officecli/officecli.exe.
        let resolved_binary = officecli_program(&config.skills.builtin_tools);
        self.record_tool_event_data(
            &call_id,
            BuiltinTool::OfficeCli.name(),
            "started",
            SkillToolEventData {
                cwd: Some(normalize_display_path(&cwd)),
                preview: Some(command_preview.clone()),
                ..SkillToolEventData::default()
            },
        )
        .await;

        let started = Instant::now();
        let mut command = Command::new(&resolved_binary);
        command
            .args(&command_args)
            .current_dir(&cwd)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        configure_bundled_tool_path(&mut command);
        configure_skill_command(&mut command);

        let disable_truncation = should_disable_output_truncation(&program, &command_args);
        let output = match execute_skill_command(
            &mut command,
            timeout_ms,
            max_output_bytes,
            disable_truncation,
            Some(SkillStreamEmitter {
                service: self.clone(),
                call_id: call_id.clone(),
                tool: BuiltinTool::OfficeCli.name().to_string(),
                kind: "stdoutDelta",
            }),
            Some(SkillStreamEmitter {
                service: self.clone(),
                call_id: call_id.clone(),
                tool: BuiltinTool::OfficeCli.name().to_string(),
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

        let timed_out = output.timed_out;
        if timed_out {
            self.record_tool_event_data(
                &call_id,
                BuiltinTool::OfficeCli.name(),
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
                    "tool": BuiltinTool::OfficeCli.name(),
                    "command": command_preview,
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
            "tool": BuiltinTool::OfficeCli.name(),
            "command": command_preview,
            "cwd": normalize_display_path(&cwd),
            "exitCode": exit_code,
            "durationMs": duration_ms,
            "stdoutTruncated": output.stdout.truncated,
            "stderrTruncated": output.stderr.truncated
        });
        self.record_tool_event_data(
            &call_id,
            BuiltinTool::OfficeCli.name(),
            "finished",
            SkillToolEventData {
                status: Some(if status.success() { "completed".to_string() } else { "failed".to_string() }),
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
                    BuiltinTool::OfficeCli,
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




