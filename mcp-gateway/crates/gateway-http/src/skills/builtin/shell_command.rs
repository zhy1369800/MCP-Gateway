fn shell_command_tool_definition(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Value {
    let mut desc = render_builtin_tool_description(BuiltinTool::ShellCommand, os, now, cfg.task_planning, cfg.read_file);
    if !cfg.shell_env.is_empty() {
        desc.push_str("\n\nUser-configured environment variables available in this terminal session:\n");
        for key in cfg.shell_env.keys() {
            desc.push_str(&format!("- {key}\n"));
        }
    }
    json!({
            "name": BuiltinTool::ShellCommand.name(),
            "description": desc,
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["exec"],
                "properties": {
                    "exec": {
                        "type": "string",
                        "description": "Shell command to run."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Concrete working directory for the operation. It must be inside one configured allowed directory. Required when more than one allowed directory exists; do not omit it in that case."
                    },
                    "timeoutMs": {
                        "type": "integer",
                        "minimum": 1000,
                        "description": "Optional command timeout in milliseconds."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for every non-documentation call. First read the complete builtin://shell_command/SKILL.md without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Calls without the correct token fail and must be retried."
                    },
                    "writes": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of file paths the command will modify. Providing this makes the gateway serialize the call against other builtin tool calls (multi_edit_file, read_file, shell_command) touching the same paths, preventing lost updates from concurrent writes. Leave empty for read-only commands."
                    }
                }
            }
    })
}

impl SkillsService {
    async fn handle_builtin_shell_command(
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
            let mut result = builtin_skill_doc_result(
                tool,
                &command_preview,
                matched_path,
                builtin_skill_token(tool),
                Self::planning_enabled(config),
            );
            // Append user-configured env vars info to the SKILL.md response
            if !config.skills.builtin_tools.shell_env.is_empty() {
                let env_section = format!(
                    "\n\n## User Environment Variables\nThe following environment variables are pre-configured and available in every shell session:\n{}\nYou can use these variables directly in commands without needing to set them.",
                    config.skills.builtin_tools.shell_env.keys()
                        .map(|k| format!("- `{k}`"))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                result.text.push_str(&env_section);
            }
            return Ok(result);
        }

        if let Some(result) = validate_skill_token_result(
            BuiltinTool::ShellCommand.name(),
            &builtin_skill_token(BuiltinTool::ShellCommand),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        if let Some(result) = self
            .check_planning_gate(
                config,
                planning_scope,
                BuiltinTool::ShellCommand,
                args.planning_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        let cwd = match resolve_builtin_cwd(
            BuiltinTool::ShellCommand,
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
        let command_args = tokens[1..].to_vec();

        // Block officecli commands from being executed through shell_command when
        // the dedicated officecli tool is enabled — force AI to use the proper tool.
        if config.skills.builtin_tools.office_cli {
            let normalized = program.to_lowercase();
            let stem = std::path::Path::new(&normalized)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if stem == "officecli" {
                return Ok(tool_error(
                    "officecli commands must be executed through the dedicated `officecli` tool, not `shell_command`. Use the `officecli` tool instead.".to_string(),
                    json!({
                        "status": "blocked",
                        "reason": "officecli is a dedicated built-in tool; do not run it via shell_command",
                        "tool": BuiltinTool::ShellCommand.name(),
                        "command": command_preview,
                        "cwd": normalize_display_path(&cwd),
                        "policyAction": "deny",
                        "redirectTo": BuiltinTool::OfficeCli.name()
                    }),
                ));
            }
        }

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
                        "tool": BuiltinTool::ShellCommand.name(),
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
                        "builtin:shell",
                        "Shell Command",
                        &tokens,
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
                            BuiltinTool::ShellCommand.name(),
                            &confirmation_id,
                            false,
                        ));
                    }
                    ConfirmationWaitOutcome::TimedOut => {
                        return Ok(confirmation_rejected_result(
                            BuiltinTool::ShellCommand.name(),
                            &confirmation_id,
                            true,
                        ));
                    }
                }
            }
            PolicyDecision::Allow => {}
        }

        // If the caller declared the paths this shell command will write,
        // serialize against the same per-path lock that multi_edit_file and
        // read_file use. Commands that leave `writes` empty keep the
        // previous concurrent behavior; declaring paths trades a bit of
        // parallelism for consistency with the file-edit tools.
        let shell_locks = if args.writes.is_empty() {
            Vec::new()
        } else {
            let mut write_targets: Vec<PathBuf> = Vec::with_capacity(args.writes.len());
            for raw in &args.writes {
                write_targets.push(resolve_file_operation_path(&cwd, raw)?);
            }
            let mut seen = std::collections::BTreeSet::new();
            write_targets.retain(|path| seen.insert(normalize_display_path(path)));
            self.acquire_file_locks(&write_targets).await
        };
        let _shell_locks = shell_locks;

        let timeout_ms = args
            .timeout_ms
            .unwrap_or(config.skills.execution.timeout_ms)
            .max(1000);
        let max_output_bytes = config.skills.execution.max_output_bytes.max(1024);
        let (runner, runner_args) = shell_command_for_current_os(&command_preview);
        self.record_tool_event_data(
            &call_id,
            BuiltinTool::ShellCommand.name(),
            "started",
            SkillToolEventData {
                cwd: Some(normalize_display_path(&cwd)),
                preview: Some(command_preview.clone()),
                ..SkillToolEventData::default()
            },
        )
        .await;

        let started = Instant::now();
        let mut command = Command::new(&runner);
        command
            .args(&runner_args)
            .current_dir(&cwd)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        configure_bundled_tool_path(&mut command);
        configure_skill_command(&mut command);

        // Inject user-configured shell environment variables
        for (key, value) in &config.skills.builtin_tools.shell_env {
            command.env(key, value);
        }

        let disable_truncation = should_disable_output_truncation(&program, &command_args);
        let output = match execute_skill_command(
            &mut command,
            timeout_ms,
            max_output_bytes,
            disable_truncation,
            Some(SkillStreamEmitter {
                service: self.clone(),
                call_id: call_id.clone(),
                tool: BuiltinTool::ShellCommand.name().to_string(),
                kind: "stdoutDelta",
            }),
            Some(SkillStreamEmitter {
                service: self.clone(),
                call_id: call_id.clone(),
                tool: BuiltinTool::ShellCommand.name().to_string(),
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
                BuiltinTool::ShellCommand.name(),
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
                    "tool": BuiltinTool::ShellCommand.name(),
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
            "tool": BuiltinTool::ShellCommand.name(),
            "command": command_preview,
            "cwd": normalize_display_path(&cwd),
            "exitCode": exit_code,
            "durationMs": duration_ms,
            "stdoutTruncated": output.stdout.truncated,
            "stderrTruncated": output.stderr.truncated
        });
        self.record_tool_event_data(
            &call_id,
            BuiltinTool::ShellCommand.name(),
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
                    BuiltinTool::ShellCommand,
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
