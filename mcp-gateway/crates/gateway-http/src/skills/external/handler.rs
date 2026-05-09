impl SkillsService {
    async fn handle_skill_command(
        &self,
        config: &GatewayConfig,
        tool_name: &str,
        skill: &DiscoveredSkill,
        args: SkillCommandArgs,
    ) -> Result<ToolResult, AppError> {
        let command_preview = args.exec.trim().to_string();
        if command_preview.is_empty() {
            return Err(AppError::BadRequest("exec cannot be empty".to_string()));
        }

        if is_external_skill_doc_read_command(&command_preview, skill) {
            let skill_md_path = skill.path.join("SKILL.md");
            let content = std::fs::read_to_string(&skill_md_path)?;
            let token = skill_token_from_content(&content);
            return Ok(skill_doc_result(
                tool_name,
                &skill.skill,
                &command_preview,
                normalize_display_path(&skill_md_path),
                content,
                token,
            ));
        }

        let expected_token = external_skill_token(skill)?;
        if let Some(result) =
            validate_skill_token_result(tool_name, &expected_token, args.skill_token.as_deref())
        {
            return Ok(result);
        }

        let tokens = split_shell_tokens(&command_preview);
        if tokens.is_empty() {
            return Err(AppError::BadRequest("exec cannot be empty".to_string()));
        }
        let program = tokens[0].clone();
        let command_args = tokens[1..].to_vec();
        let display_name = skill_display_name(skill).to_string();

        let skill_md_path = skill.path.join("SKILL.md");
        let policy = evaluate_policy(
            &config.skills,
            &program,
            &command_args,
            &command_preview,
            &skill_md_path,
            None,
        );
        match policy {
            PolicyDecision::Deny(reason) => {
                return Ok(tool_error(
                    mcp_gateway_policy_denied_text(&reason),
                    json!({
                        "status": "blocked",
                        "reason": reason,
                        "tool": tool_name,
                        "command": command_preview,
                        "policyAction": "deny",
                        "policyHelp": mcp_gateway_policy_denied_help()
                    }),
                ));
            }
            PolicyDecision::Confirm(reason) => {
                let (confirmation_id, already_decided) = match self
                    .create_confirmation_with_metadata(
                        &skill.skill,
                        &display_name,
                        &tokens,
                        &command_preview,
                        &reason,
                        ConfirmationMetadata {
                            kind: "skill".to_string(),
                            cwd: normalize_display_path(&skill.path),
                            affected_paths: Vec::new(),
                            preview: command_preview.clone(),
                        },
                    )
                    .await
                {
                    // 全新确认 → 正常走等待流程
                    CreateConfirmationResult::Created(c) => (c.id, None),
                    // 同指纹已有 Pending → 复用同一个 id，继续等待
                    CreateConfirmationResult::Reused(c) => (c.id, None),
                    // 同指纹刚超时 → 直接拒绝，不再弹窗
                    CreateConfirmationResult::AlreadyTimedOut(id) => {
                        (id.clone(), Some(ConfirmationWaitOutcome::TimedOut))
                    }
                };

                let outcome = match already_decided {
                    Some(decided) => decided,
                    None => {
                        self.wait_for_confirmation_decision(
                            &confirmation_id,
                            Self::CONFIRMATION_DECISION_TIMEOUT,
                            Duration::from_millis(250),
                        )
                        .await
                    }
                };

                match outcome {
                    ConfirmationWaitOutcome::Approved => {}
                    ConfirmationWaitOutcome::Rejected => {
                        return Ok(confirmation_rejected_result(
                            tool_name,
                            &confirmation_id,
                            false,
                        ));
                    }
                    ConfirmationWaitOutcome::TimedOut => {
                        return Ok(confirmation_rejected_result(
                            tool_name,
                            &confirmation_id,
                            true,
                        ));
                    }
                }
            }
            PolicyDecision::Allow => {}
        }

        let timeout_ms = config.skills.execution.timeout_ms.max(1000);
        let max_output_bytes = config.skills.execution.max_output_bytes.max(1024);
        let (runner, runner_args) = shell_command_for_current_os(&command_preview);

        let started = Instant::now();
        let mut command = Command::new(&runner);
        command
            .args(&runner_args)
            .current_dir(&skill.path)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        configure_bundled_tool_path(&mut command);
        configure_skill_command(&mut command);

        let disable_truncation = should_disable_output_truncation(&program, &command_args);
        let output = execute_skill_command(
            &mut command,
            timeout_ms,
            max_output_bytes,
            disable_truncation,
            None,
            None,
        )
        .await?;
        let duration_ms = started.elapsed().as_millis() as u64;
        let stdout = output.stdout.text;
        let stderr = output.stderr.text;
        let stdout_truncated = output.stdout.truncated;
        let stderr_truncated = output.stderr.truncated;
        let exit_code = output.status.code().unwrap_or(-1);

        let mut structured = serde_json::Map::new();
        structured.insert(
            "status".to_string(),
            Value::String(if output.status.success() {
                "completed".to_string()
            } else {
                "failed".to_string()
            }),
        );
        structured.insert("tool".to_string(), Value::String(tool_name.to_string()));
        structured.insert("skill".to_string(), Value::String(skill.skill.clone()));
        structured.insert("command".to_string(), Value::String(command_preview));
        structured.insert("exitCode".to_string(), json!(exit_code));
        structured.insert("durationMs".to_string(), json!(duration_ms));
        structured.insert("stdoutTruncated".to_string(), Value::Bool(stdout_truncated));
        structured.insert("stderrTruncated".to_string(), Value::Bool(stderr_truncated));
        let structured = Value::Object(structured);

        let output_text = command_output_text(&stdout, &stderr);

        if output.status.success() {
            Ok(tool_success(output_text, structured))
        } else {
            Ok(tool_error(
                command_failure_text(exit_code, &stdout, &stderr),
                structured,
            ))
        }
    }

    async fn discover_skills(
        &self,
        skills_config: &SkillsConfig,
    ) -> Result<Vec<DiscoveredSkill>, AppError> {
        let roots = skills_config.roots.clone();
        let signature = roots_signature(&roots);

        {
            let now = Instant::now();
            let guard = self.discovery_cache.read().await;
            if let Some(cached) = guard.as_ref() {
                if cached.signature == signature && now <= cached.expires_at {
                    return Ok(cached.discovered.clone());
                }
            }
        }

        let roots_for_scan = roots.clone();
        let discovered = tokio::task::spawn_blocking(move || discover_skills_sync(&roots_for_scan))
            .await
            .map_err(|error| {
                AppError::Internal(format!("skills discovery join error: {error}"))
            })??;

        {
            let mut guard = self.discovery_cache.write().await;
            *guard = Some(SkillDiscoveryCache {
                signature,
                discovered: discovered.clone(),
                expires_at: Instant::now() + Self::SKILL_DISCOVERY_CACHE_TTL,
            });
        }

        Ok(discovered)
    }
}
