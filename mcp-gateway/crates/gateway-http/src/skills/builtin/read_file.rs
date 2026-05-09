fn read_file_tool_definition(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Value {
    json!({
            "name": BuiltinTool::ReadFile.name(),
            "description": render_builtin_tool_description(BuiltinTool::ReadFile, os, now, cfg.task_planning, cfg.read_file),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Text file path to read. Relative paths resolve from cwd. Use builtin://<skill>/SKILL.md to read bundled skill documentation."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Concrete working directory for relative paths. It must be inside one configured allowed directory. Required when more than one allowed directory exists."
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional 1-based starting line number. Defaults to 1."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 2000,
                        "description": "Optional number of lines to return. Defaults to 2000, maximum 2000."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for normal file reads. First read the complete builtin://read_file/SKILL.md without skillToken, then use the returned skillToken. Documentation reads do not require it."
                    }
                }
            }
    })
}

impl SkillsService {
    async fn handle_builtin_read_file(
        &self,
        config: &GatewayConfig,
        args: ReadFileArgs,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
        let call_id = Uuid::new_v4().to_string();
        let requested_path = args.path.trim();
        if requested_path.is_empty() {
            return Err(AppError::BadRequest("path cannot be empty".to_string()));
        }

        if let Some((tool, matched_path)) = builtin_skill_doc_arg(requested_path) {
            return Ok(builtin_skill_read_doc_result(
                tool,
                matched_path,
                builtin_skill_token(tool),
                Self::planning_enabled(config),
            ));
        }

        if let Some(result) = validate_skill_token_result(
            BuiltinTool::ReadFile.name(),
            &builtin_skill_token(BuiltinTool::ReadFile),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        if let Some(result) = self
            .check_planning_gate(
                config,
                planning_scope,
                BuiltinTool::ReadFile,
                args.planning_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        let cwd =
            match resolve_builtin_cwd(BuiltinTool::ReadFile, &config.skills, args.cwd.as_deref()) {
                Ok(value) => value,
                Err(result) => return Ok(result),
            };
        let target = resolve_file_operation_path(&cwd, requested_path)?;
        self.record_tool_event_data(
            &call_id,
            BuiltinTool::ReadFile.name(),
            "started",
            SkillToolEventData {
                cwd: Some(normalize_display_path(&cwd)),
                affected_paths: vec![normalize_display_path(&target)],
                preview: Some(read_file_window_preview(args.offset, args.limit)),
                ..SkillToolEventData::default()
            },
        )
        .await;

        match evaluate_paths_policy(&config.skills, std::slice::from_ref(&target)) {
            PolicyDecision::Deny(reason) => {
                return Ok(tool_error(
                    mcp_gateway_policy_denied_text(&reason),
                    json!({
                        "status": "blocked",
                        "reason": reason,
                        "tool": BuiltinTool::ReadFile.name(),
                        "cwd": normalize_display_path(&cwd),
                        "policyAction": "deny",
                        "policyHelp": mcp_gateway_policy_denied_help(),
                        "affectedPaths": [normalize_display_path(&target)]
                    }),
                ));
            }
            PolicyDecision::Confirm(reason) => {
                let metadata = ConfirmationMetadata {
                    kind: "read".to_string(),
                    cwd: normalize_display_path(&cwd),
                    affected_paths: vec![normalize_display_path(&target)],
                    preview: format!("Read {}", normalize_display_path(&target)),
                };
                let confirmation_id = match self
                    .create_confirmation_with_metadata(
                        "builtin:read_file",
                        "Read File",
                        &[String::from("read_file")],
                        requested_path,
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
                        return Ok(tool_error(
                            format!("read_file rejected by user: {reason}"),
                            json!({
                                "status": "rejected",
                                "reason": reason,
                                "tool": BuiltinTool::ReadFile.name(),
                                "confirmationId": confirmation_id,
                                "affectedPaths": [normalize_display_path(&target)]
                            }),
                        ));
                    }
                    ConfirmationWaitOutcome::TimedOut => {
                        return Ok(tool_error(
                            format!("read_file confirmation timed out: {reason}"),
                            json!({
                                "status": "timeout",
                                "reason": reason,
                                "tool": BuiltinTool::ReadFile.name(),
                                "confirmationId": confirmation_id,
                                "affectedPaths": [normalize_display_path(&target)]
                            }),
                        ));
                    }
                }
            }
            PolicyDecision::Allow => {}
        }

        match read_text_file_window(&target, args.offset, args.limit) {
            Ok(output) => {
                let text = if output.empty {
                    format!(
                        "File exists but is empty.\nPath: {}",
                        normalize_display_path(&target)
                    )
                } else {
                    output.numbered_content.clone()
                };
                let structured = json!({
                    "status": "completed",
                    "tool": BuiltinTool::ReadFile.name(),
                    "path": normalize_display_path(&target),
                    "cwd": normalize_display_path(&cwd),
                    "startLine": output.start_line,
                    "endLine": output.end_line,
                    "limit": output.limit,
                    "numLines": output.num_lines,
                    "totalLines": output.total_lines,
                    "totalBytes": output.total_bytes,
                    "truncated": output.truncated,
                    "lineTruncated": output.line_truncated,
                    "empty": output.empty,
                    "content": output.numbered_content.clone(),
                    "lineNumberFormat": "line_number<TAB>content"
                });
                self.record_tool_event_data(
                    &call_id,
                    BuiltinTool::ReadFile.name(),
                    "finished",
                    SkillToolEventData {
                        status: Some("completed".to_string()),
                        affected_paths: vec![normalize_display_path(&target)],
                        text: Some(format!("{} lines", output.num_lines)),
                        ..SkillToolEventData::default()
                    },
                )
                .await;
                Ok(tool_success_with_planning_reminder(
                    text,
                    structured,
                    self.planning_success_hints(
                        config,
                        planning_scope,
                        args.planning_id.as_deref(),
                        BuiltinTool::ReadFile,
                        None,
                    )
                    .await,
                ))
            }
            Err(error) => {
                self.record_tool_event_data(
                    &call_id,
                    BuiltinTool::ReadFile.name(),
                    "finished",
                    SkillToolEventData {
                        status: Some("failed".to_string()),
                        affected_paths: vec![normalize_display_path(&target)],
                        text: Some(error.to_string()),
                        ..SkillToolEventData::default()
                    },
                )
                .await;
                Ok(tool_error(
                    error.to_string(),
                    json!({
                        "status": "failed",
                        "tool": BuiltinTool::ReadFile.name(),
                        "path": normalize_display_path(&target),
                        "cwd": normalize_display_path(&cwd),
                        "message": error.to_string()
                    }),
                ))
            }
        }
    }
}

const READ_FILE_DEFAULT_LIMIT: usize = 2000;
const READ_FILE_MAX_LIMIT: usize = 2000;
const READ_FILE_MAX_BYTES: u64 = 10 * 1024 * 1024;
const READ_FILE_BINARY_PROBE_BYTES: usize = 8192;
const READ_FILE_MAX_LINE_CHARS: usize = 4000;

#[derive(Debug)]
struct ReadFileWindow {
    numbered_content: String,
    start_line: usize,
    end_line: usize,
    limit: usize,
    num_lines: usize,
    total_lines: usize,
    total_bytes: u64,
    truncated: bool,
    line_truncated: bool,
    empty: bool,
}

fn read_file_window_preview(offset: Option<usize>, limit: Option<usize>) -> String {
    let start = offset.unwrap_or(1).max(1);
    let limit = limit
        .unwrap_or(READ_FILE_DEFAULT_LIMIT)
        .min(READ_FILE_MAX_LIMIT);
    format!(
        "lines {start}-{}",
        start.saturating_add(limit).saturating_sub(1)
    )
}

fn read_text_file_window(
    target: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ReadFileWindow, AppError> {
    if !target.exists() {
        return Err(AppError::BadRequest(format!(
            "file does not exist: {}",
            target.to_string_lossy()
        )));
    }
    if target.is_dir() {
        return Err(AppError::BadRequest(format!(
            "read_file target is a directory: {}",
            target.to_string_lossy()
        )));
    }

    let metadata = fs::metadata(target)?;
    if metadata.len() > READ_FILE_MAX_BYTES {
        return Err(AppError::BadRequest(format!(
            "file is too large to read safely: {} bytes (max {} bytes)",
            metadata.len(),
            READ_FILE_MAX_BYTES
        )));
    }
    if is_probably_binary_file(target)? {
        return Err(AppError::BadRequest(format!(
            "file appears to be binary and cannot be read as text: {}",
            target.to_string_lossy()
        )));
    }

    let content = fs::read_to_string(target).map_err(|error| {
        AppError::BadRequest(format!(
            "failed to read file as UTF-8 text: {} ({error})",
            target.to_string_lossy()
        ))
    })?;
    let total_bytes = metadata.len();
    let normalized = normalize_to_lf(&content);
    if normalized.is_empty() {
        return Ok(ReadFileWindow {
            numbered_content: String::new(),
            start_line: 1,
            end_line: 0,
            limit: limit
                .unwrap_or(READ_FILE_DEFAULT_LIMIT)
                .min(READ_FILE_MAX_LIMIT),
            num_lines: 0,
            total_lines: 0,
            total_bytes,
            truncated: false,
            line_truncated: false,
            empty: true,
        });
    }

    let lines = normalized.lines().collect::<Vec<_>>();
    let total_lines = lines.len();
    let start_line = offset.unwrap_or(1).max(1);
    let limit = limit
        .unwrap_or(READ_FILE_DEFAULT_LIMIT)
        .min(READ_FILE_MAX_LIMIT);
    let start_index = start_line.saturating_sub(1).min(total_lines);
    let end_index = start_index.saturating_add(limit).min(total_lines);
    let mut line_truncated = false;
    let numbered = lines[start_index..end_index]
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let line_no = start_index + index + 1;
            let (display_line, truncated) = truncate_line_for_read(line);
            line_truncated |= truncated;
            format!("{line_no}\t{display_line}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let num_lines = end_index.saturating_sub(start_index);
    let end_line = if num_lines == 0 {
        start_line.saturating_sub(1)
    } else {
        start_index + num_lines
    };

    Ok(ReadFileWindow {
        numbered_content: numbered,
        start_line,
        end_line,
        limit,
        num_lines,
        total_lines,
        total_bytes,
        truncated: end_index < total_lines,
        line_truncated,
        empty: false,
    })
}

fn is_probably_binary_file(path: &Path) -> Result<bool, AppError> {
    let mut file = fs::File::open(path)?;
    let mut buffer = [0_u8; READ_FILE_BINARY_PROBE_BYTES];
    let bytes_read = file.read(&mut buffer)?;
    Ok(buffer[..bytes_read].contains(&0))
}

fn truncate_line_for_read(line: &str) -> (String, bool) {
    if line.chars().count() <= READ_FILE_MAX_LINE_CHARS {
        return (line.to_string(), false);
    }
    let mut value = line
        .chars()
        .take(READ_FILE_MAX_LINE_CHARS)
        .collect::<String>();
    value.push_str(" [line truncated]");
    (value, true)
}

