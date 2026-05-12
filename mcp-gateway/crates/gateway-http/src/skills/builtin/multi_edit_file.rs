fn multi_edit_file_tool_definition(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Value {
    json!({
            "name": BuiltinTool::MultiEditFile.name(),
            "description": render_builtin_tool_description(BuiltinTool::MultiEditFile, os, now, cfg.task_planning, cfg.read_file),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": [],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Legacy single-file edit path, relative to cwd unless absolute. Use with top-level edits."
                    },
                    "edits": {
                        "type": "array",
                        "description": "Legacy ordered exact replacements for path. All edits are validated and applied in memory before writing.",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["old_string", "new_string"],
                            "properties": {
                                "old_string": {
                                    "type": "string",
                                    "description": "Exact text to replace. Must match the current file after LF normalization."
                                },
                                "new_string": {
                                    "type": "string",
                                    "description": "Replacement text. Must be different from old_string."
                                },
                                "replace_all": {
                                    "type": "boolean",
                                    "description": "Replace every occurrence of old_string in the current in-memory file state. Defaults to false."
                                },
                                "startLine": {
                                    "type": "integer",
                                    "minimum": 1,
                                    "description": "Optional 1-based hint for the expected starting line. Used to choose the closest match when old_string is not globally unique."
                                }
                            }
                        }
                    },
                    "files": {
                        "type": "array",
                        "description": "Multiple existing files to edit. Each item has path and edits.",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["path", "edits"],
                            "properties": {
                                "path": {"type": "string"},
                                "edits": {
                                    "type": "array",
                                    "minItems": 1,
                                    "items": {
                                        "type": "object",
                                        "additionalProperties": false,
                                        "required": ["old_string", "new_string"],
                                        "properties": {
                                            "old_string": {"type": "string"},
                                            "new_string": {"type": "string"},
                                            "replace_all": {"type": "boolean"},
                                            "startLine": {"type": "integer", "minimum": 1}
                                        }
                                    }
                                }
                            }
                        }
                    },
                    "operations": {
                        "type": "array",
                        "description": "Structured file operations. Supported type values: edit, create, delete, move. The gateway validates every operation before committing writes.",
                        "items": {
                            "type": "object",
                            "additionalProperties": true,
                            "required": ["type"],
                            "properties": {
                                "type": {"type": "string", "enum": ["edit", "create", "delete", "move"]},
                                "path": {"type": "string"},
                                "from": {"type": "string"},
                                "to": {"type": "string"},
                                "content": {"type": "string"},
                                "overwrite": {"type": "boolean"},
                                "edits": {"type": "array"}
                            }
                        }
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Concrete working directory for relative paths. It must be inside one configured allowed directory. Required when more than one allowed directory exists; do not omit it in that case."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for every multi_edit_file call. First read the complete builtin://multi_edit_file/SKILL.md with read_file when available (or shell_command as fallback) without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Calls without the correct token fail and must be retried."
                    }
                }
            }
    })
}

impl SkillsService {
    async fn handle_builtin_multi_edit_file(
        &self,
        config: &GatewayConfig,
        args: MultiEditFileArgs,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
        let call_id = Uuid::new_v4().to_string();
        if let Some(result) = validate_skill_token_result(
            BuiltinTool::MultiEditFile.name(),
            &builtin_skill_token(BuiltinTool::MultiEditFile),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        if let Some(result) = self
            .check_planning_gate(
                config,
                planning_scope,
                BuiltinTool::MultiEditFile,
                args.planning_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        let cwd = match resolve_builtin_cwd(
            BuiltinTool::MultiEditFile,
            &config.skills,
            args.cwd.as_deref(),
        ) {
            Ok(cwd) => cwd,
            Err(result) => return Ok(result),
        };

        let operations = normalize_multi_edit_operations(&args)?;
        let affected_paths = multi_edit_affected_paths(&cwd, &operations)?;
        let preview = multi_edit_preview(&operations);
        self.record_tool_event_data(
            &call_id,
            BuiltinTool::MultiEditFile.name(),
            "editPreview",
            SkillToolEventData {
                cwd: Some(normalize_display_path(&cwd)),
                preview: Some(truncate_preview(&preview, 4000)),
                affected_paths: affected_paths
                    .iter()
                    .map(|path| normalize_display_path(path))
                    .collect(),
                changes: Some(multi_edit_preview_changes(&operations)),
                ..SkillToolEventData::default()
            },
        )
        .await;

        let access_decision = evaluate_paths_policy(&config.skills, &affected_paths);
        match access_decision {
            PolicyDecision::Deny(reason) => {
                return Ok(tool_error(
                    mcp_gateway_policy_denied_text(&reason),
                    json!({
                        "status": "blocked",
                        "reason": reason,
                        "tool": BuiltinTool::MultiEditFile.name(),
                        "cwd": normalize_display_path(&cwd),
                        "policyAction": "deny",
                        "policyHelp": mcp_gateway_policy_denied_help(&reason),
                        "affectedPaths": affected_paths.iter().map(|path| normalize_display_path(path)).collect::<Vec<_>>()
                    }),
                ));
            }
            PolicyDecision::Confirm { reason, reason_key } => {
                let metadata = ConfirmationMetadata {
                    kind: "edit".to_string(),
                    cwd: normalize_display_path(&cwd),
                    affected_paths: affected_paths
                        .iter()
                        .map(|path| normalize_display_path(path))
                        .collect(),
                    preview: truncate_preview(&preview, 4000),
                    reason_key,
                };
                let confirmation_id = match self
                    .create_confirmation_with_metadata(
                        "builtin:multi_edit_file",
                        "Multi Edit File",
                        &[String::from("multi_edit_file")],
                        &preview,
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
                            BuiltinTool::MultiEditFile.name(),
                            &confirmation_id,
                            false,
                        ));
                    }
                    ConfirmationWaitOutcome::TimedOut => {
                        return Ok(confirmation_rejected_result(
                            BuiltinTool::MultiEditFile.name(),
                            &confirmation_id,
                            true,
                        ));
                    }
                }
            }
            PolicyDecision::Allow => {}
        }

        match apply_multi_edit_file(&cwd, &operations) {
            Ok(outcome) => {
                let text = file_edit_summary_text(&outcome.summary);
                self.record_tool_event_data(
                    &call_id,
                    BuiltinTool::MultiEditFile.name(),
                    "finished",
                    SkillToolEventData {
                        status: Some("completed".to_string()),
                        delta: Some(outcome.delta.clone()),
                        warnings: outcome.warnings.clone(),
                        ..SkillToolEventData::default()
                    },
                )
                .await;
                Ok(tool_success_with_planning_reminder(
                    text,
                    json!({
                        "status": "completed",
                        "tool": BuiltinTool::MultiEditFile.name(),
                        "cwd": normalize_display_path(&cwd),
                        "added": outcome.summary.added,
                        "modified": outcome.summary.modified,
                        "deleted": outcome.summary.deleted,
                        "moved": outcome.summary.moved,
                        "delta": edit_delta_for_model(&outcome.delta),
                        "warnings": outcome.warnings
                    }),
                    self.planning_success_hints(
                        config,
                        planning_scope,
                        args.planning_id.as_deref(),
                        BuiltinTool::MultiEditFile,
                        None,
                    )
                    .await,
                ))
            }
            Err(failure) => {
                self.record_tool_event_data(
                    &call_id,
                    BuiltinTool::MultiEditFile.name(),
                    "finished",
                    SkillToolEventData {
                        status: Some("failed".to_string()),
                        delta: Some(failure.delta.clone()),
                        ..SkillToolEventData::default()
                    },
                )
                .await;
                Ok(tool_error_with_edit_failure_reminder(
                    failure.message.clone(),
                    json!({
                        "status": "failed",
                        "tool": BuiltinTool::MultiEditFile.name(),
                        "cwd": normalize_display_path(&cwd),
                        "message": failure.message,
                        "delta": edit_delta_for_model(&failure.delta)
                    }),
                    self.planning_edit_failure_reminder(
                        config,
                        planning_scope,
                        args.planning_id.as_deref(),
                        BuiltinTool::MultiEditFile,
                    )
                    .await,
                ))
            }
        }
    }
}

fn edit_failure(error: AppError, delta: &FileEditDelta) -> FileEditFailure {
    FileEditFailure {
        message: error.to_string(),
        delta: delta.clone(),
    }
}

fn edit_delta_for_model(delta: &FileEditDelta) -> Value {
    let changes = delta
        .changes
        .iter()
        .map(|change| match change {
            FileEditChange::Add {
                path,
                content,
                overwritten_content,
            } => json!({
                "kind": "add",
                "path": path,
                "contentBytes": content.len(),
                "overwritten": overwritten_content.is_some(),
                "overwrittenContentBytes": overwritten_content.as_ref().map(String::len)
            }),
            FileEditChange::Delete { path, content } => json!({
                "kind": "delete",
                "path": path,
                "contentAvailable": content.is_some(),
                "contentBytes": content.as_ref().map(String::len)
            }),
            FileEditChange::Update {
                path,
                move_path,
                old_content,
                new_content,
                overwritten_move_content,
            } => json!({
                "kind": "update",
                "path": path,
                "movePath": move_path,
                "oldContentBytes": old_content.len(),
                "newContentBytes": new_content.len(),
                "overwrittenMoveContent": overwritten_move_content.is_some(),
                "overwrittenMoveContentBytes": overwritten_move_content.as_ref().map(String::len)
            }),
        })
        .collect::<Vec<_>>();

    json!({
        "exact": delta.exact,
        "changeCount": delta.changes.len(),
        "changes": changes
    })
}

#[derive(Debug)]
enum StagedFileOperation {
    Update {
        path: PathBuf,
        old_content: String,
        new_content: String,
    },
    Create {
        path: PathBuf,
        content: String,
        overwritten_content: Option<String>,
    },
    Delete {
        path: PathBuf,
        content: String,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
        content: String,
        overwritten_content: Option<String>,
    },
}

fn normalize_multi_edit_operations(
    args: &MultiEditFileArgs,
) -> Result<Vec<MultiEditFileOperation>, AppError> {
    let mut operations = Vec::new();

    if let Some(path) = args
        .path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        operations.push(MultiEditFileOperation::Edit {
            path: path.to_string(),
            edits: args.edits.clone(),
        });
    } else if !args.edits.is_empty() {
        return Err(AppError::BadRequest(
            "path is required when using top-level edits".to_string(),
        ));
    }

    for file in &args.files {
        operations.push(MultiEditFileOperation::Edit {
            path: file.path.clone(),
            edits: file.edits.clone(),
        });
    }

    operations.extend(args.operations.clone());

    if operations.is_empty() {
        return Err(AppError::BadRequest(
            "multi_edit_file requires path+edits, files, or operations".to_string(),
        ));
    }

    Ok(operations)
}

fn multi_edit_affected_paths(
    cwd: &Path,
    operations: &[MultiEditFileOperation],
) -> Result<Vec<PathBuf>, AppError> {
    let mut paths = Vec::new();
    for operation in operations {
        match operation {
            MultiEditFileOperation::Edit { path, .. }
            | MultiEditFileOperation::Create { path, .. }
            | MultiEditFileOperation::Delete { path } => {
                paths.push(resolve_file_operation_path(cwd, path)?);
            }
            MultiEditFileOperation::Move { from, to, .. } => {
                paths.push(resolve_file_operation_path(cwd, from)?);
                paths.push(resolve_file_operation_path(cwd, to)?);
            }
        }
    }

    let mut seen = BTreeSet::new();
    paths.retain(|path| seen.insert(normalize_display_path(path)));
    Ok(paths)
}

fn apply_multi_edit_file(
    cwd: &Path,
    operations: &[MultiEditFileOperation],
) -> Result<FileEditOutcome, FileEditFailure> {
    let empty_delta = FileEditDelta::default();
    let mut touched_paths = BTreeSet::new();
    let mut staged = Vec::new();
    let mut warnings = Vec::new();

    for (index, operation) in operations.iter().enumerate() {
        stage_multi_edit_operation(
            cwd,
            operation,
            index + 1,
            &mut touched_paths,
            &mut staged,
            &mut warnings,
        )
        .map_err(|error| edit_failure(error, &empty_delta))?;
    }

    let mut delta = FileEditDelta::default();
    for operation in staged {
        commit_staged_file_operation(operation, &mut delta)
            .map_err(|error| edit_failure(error, &delta))?;
    }

    if delta.changes.is_empty() {
        return Err(edit_failure(
            AppError::BadRequest("multi_edit_file produced no changes".to_string()),
            &delta,
        ));
    }

    Ok(FileEditOutcome {
        summary: summarize_file_edit_delta(&delta),
        delta,
        warnings,
    })
}

fn stage_multi_edit_operation(
    cwd: &Path,
    operation: &MultiEditFileOperation,
    index: usize,
    touched_paths: &mut BTreeSet<String>,
    staged: &mut Vec<StagedFileOperation>,
    warnings: &mut Vec<String>,
) -> Result<(), AppError> {
    match operation {
        MultiEditFileOperation::Edit { path, edits } => {
            if edits.is_empty() {
                return Err(AppError::BadRequest(format!(
                    "operation {index} edit for {path} has empty edits"
                )));
            }
            let target = resolve_file_operation_path(cwd, path)?;
            ensure_unique_operation_path(touched_paths, &target)?;
            if target.is_dir() {
                return Err(AppError::BadRequest(format!(
                    "multi_edit_file target is a directory: {}",
                    target.to_string_lossy()
                )));
            }
            let original = fs::read_to_string(&target)?;
            let line_endings = detect_text_line_endings(&original);
            let original_lf = normalize_to_lf(&original);
            let updated_lf = apply_multi_edits_to_lf_content(&original_lf, edits, &target)?;
            if updated_lf == original_lf {
                return Err(AppError::BadRequest(format!(
                    "operation {index} edit for {path} produced no changes"
                )));
            }
            let updated = restore_line_endings(&updated_lf, line_endings);
            warnings.extend(collect_edit_warnings(&target, &original, &updated));
            staged.push(StagedFileOperation::Update {
                path: target,
                old_content: original,
                new_content: updated,
            });
        }
        MultiEditFileOperation::Create {
            path,
            content,
            overwrite,
        } => {
            let target = resolve_file_operation_path(cwd, path)?;
            ensure_unique_operation_path(touched_paths, &target)?;
            if target.is_dir() {
                return Err(AppError::BadRequest(format!(
                    "create target is a directory: {}",
                    target.to_string_lossy()
                )));
            }
            let overwritten_content = match fs::read_to_string(&target) {
                Ok(existing) => {
                    if !overwrite {
                        return Err(AppError::BadRequest(format!(
                            "create target already exists: {}",
                            target.to_string_lossy()
                        )));
                    }
                    Some(existing)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            warnings.extend(collect_edit_warnings(
                &target,
                overwritten_content.as_deref().unwrap_or_default(),
                content,
            ));
            staged.push(StagedFileOperation::Create {
                path: target,
                content: content.clone(),
                overwritten_content,
            });
        }
        MultiEditFileOperation::Delete { path } => {
            let target = resolve_file_operation_path(cwd, path)?;
            ensure_unique_operation_path(touched_paths, &target)?;
            if target.is_dir() {
                return Err(AppError::BadRequest(format!(
                    "delete target is a directory: {}",
                    target.to_string_lossy()
                )));
            }
            let content = fs::read_to_string(&target)?;
            staged.push(StagedFileOperation::Delete {
                path: target,
                content,
            });
        }
        MultiEditFileOperation::Move {
            from,
            to,
            overwrite,
        } => {
            let source = resolve_file_operation_path(cwd, from)?;
            let target = resolve_file_operation_path(cwd, to)?;
            if source == target {
                return Err(AppError::BadRequest(format!(
                    "move source and target are the same: {}",
                    source.to_string_lossy()
                )));
            }
            ensure_unique_operation_path(touched_paths, &source)?;
            ensure_unique_operation_path(touched_paths, &target)?;
            if source.is_dir() {
                return Err(AppError::BadRequest(format!(
                    "move source is a directory: {}",
                    source.to_string_lossy()
                )));
            }
            if target.is_dir() {
                return Err(AppError::BadRequest(format!(
                    "move target is a directory: {}",
                    target.to_string_lossy()
                )));
            }
            let content = fs::read_to_string(&source)?;
            let overwritten_content = match fs::read_to_string(&target) {
                Ok(existing) => {
                    if !overwrite {
                        return Err(AppError::BadRequest(format!(
                            "move target already exists: {}",
                            target.to_string_lossy()
                        )));
                    }
                    Some(existing)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            staged.push(StagedFileOperation::Move {
                from: source,
                to: target,
                content,
                overwritten_content,
            });
        }
    }

    Ok(())
}

fn ensure_unique_operation_path(
    touched_paths: &mut BTreeSet<String>,
    path: &Path,
) -> Result<(), AppError> {
    let display = normalize_display_path(path);
    if !touched_paths.insert(display.clone()) {
        return Err(AppError::BadRequest(format!(
            "multi_edit_file cannot touch the same path more than once in one call: {display}"
        )));
    }
    Ok(())
}

fn commit_staged_file_operation(
    operation: StagedFileOperation,
    delta: &mut FileEditDelta,
) -> Result<(), AppError> {
    match operation {
        StagedFileOperation::Update {
            path,
            old_content,
            new_content,
            ..
        } => {
            fs::write(&path, &new_content).map_err(|error| {
                delta.exact = false;
                AppError::from(error)
            })?;
            delta.changes.push(FileEditChange::Update {
                path: normalize_display_path(&path),
                move_path: None,
                old_content,
                new_content,
                overwritten_move_content: None,
            });
        }
        StagedFileOperation::Create {
            path,
            content,
            overwritten_content,
            ..
        } => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    delta.exact = false;
                    AppError::from(error)
                })?;
            }
            fs::write(&path, &content).map_err(|error| {
                delta.exact = false;
                AppError::from(error)
            })?;
            delta.changes.push(FileEditChange::Add {
                path: normalize_display_path(&path),
                content,
                overwritten_content,
            });
        }
        StagedFileOperation::Delete { path, content, .. } => {
            fs::remove_file(&path).map_err(|error| {
                delta.exact = false;
                AppError::from(error)
            })?;
            delta.changes.push(FileEditChange::Delete {
                path: normalize_display_path(&path),
                content: Some(content),
            });
        }
        StagedFileOperation::Move {
            from,
            to,
            content,
            overwritten_content,
            ..
        } => {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    delta.exact = false;
                    AppError::from(error)
                })?;
            }
            fs::write(&to, &content).map_err(|error| {
                delta.exact = false;
                AppError::from(error)
            })?;
            fs::remove_file(&from).map_err(|error| {
                delta.exact = false;
                AppError::from(error)
            })?;
            delta.changes.push(FileEditChange::Update {
                path: normalize_display_path(&from),
                move_path: Some(normalize_display_path(&to)),
                old_content: content.clone(),
                new_content: content,
                overwritten_move_content: overwritten_content,
            });
        }
    }
    Ok(())
}

fn summarize_file_edit_delta(delta: &FileEditDelta) -> FileEditSummary {
    let mut summary = FileEditSummary {
        added: Vec::new(),
        modified: Vec::new(),
        deleted: Vec::new(),
        moved: Vec::new(),
    };
    for change in &delta.changes {
        match change {
            FileEditChange::Add { path, .. } => summary.added.push(path.clone()),
            FileEditChange::Delete { path, .. } => summary.deleted.push(path.clone()),
            FileEditChange::Update {
                path, move_path, ..
            } => {
                if let Some(move_path) = move_path {
                    summary.moved.push(format!("{path} -> {move_path}"));
                } else {
                    summary.modified.push(path.clone());
                }
            }
        }
    }
    summary
}

fn apply_multi_edits_to_lf_content(
    original_lf: &str,
    edits: &[MultiEditFileEdit],
    path: &Path,
) -> Result<String, AppError> {
    let mut content = original_lf.to_string();
    let mut applied_new_strings = Vec::<String>::new();

    for (index, edit) in edits.iter().enumerate() {
        let old_string = normalize_to_lf(&edit.old_string);
        let new_string = normalize_to_lf(&edit.new_string);

        if old_string.is_empty() {
            return Err(AppError::BadRequest(format!(
                "edit {} for {} has empty old_string; use a create operation for new files or include surrounding existing text for insertions",
                index + 1,
                path.to_string_lossy()
            )));
        }
        if old_string == new_string {
            return Err(AppError::BadRequest(format!(
                "edit {} for {} has identical old_string and new_string",
                index + 1,
                path.to_string_lossy()
            )));
        }
        for previous_new_string in &applied_new_strings {
            if previous_new_string.contains(&old_string) {
                return Err(AppError::BadRequest(format!(
                    "edit {} old_string is a substring of a previous new_string; split or reorder edits to avoid re-editing generated content",
                    index + 1
                )));
            }
        }

        if edit.replace_all {
            let matches = content.match_indices(&old_string).count();
            if matches == 0 {
                return Err(AppError::BadRequest(format!(
                    "edit {} failed: old_string not found in {}",
                    index + 1,
                    path.to_string_lossy()
                )));
            }
            content = content.replace(&old_string, &new_string);
            applied_new_strings.push(new_string);
            continue;
        }

        let matches = content
            .match_indices(&old_string)
            .map(|(byte_index, _)| byte_index)
            .collect::<Vec<_>>();
        let Some(found) = select_multi_edit_match(&content, &old_string, &matches, edit.start_line)
        else {
            return Err(AppError::BadRequest(format!(
                "edit {} failed: old_string not found in {}",
                index + 1,
                path.to_string_lossy()
            )));
        };

        if matches.len() > 1 && edit.start_line.is_none() {
            return Err(AppError::BadRequest(format!(
                "edit {} found {} matches in {}; set replace_all=true or provide startLine to disambiguate",
                index + 1,
                matches.len(),
                path.to_string_lossy()
            )));
        }

        let end = found + old_string.len();
        content.replace_range(found..end, &new_string);
        applied_new_strings.push(new_string);
    }

    Ok(content)
}

fn select_multi_edit_match(
    content: &str,
    old_string: &str,
    matches: &[usize],
    start_line: Option<usize>,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    if matches.len() == 1 || start_line.is_none() {
        return matches.first().copied();
    }

    let expected = start_line.unwrap_or(1);
    let old_line_count = old_string.split('\n').count().max(1);
    matches.iter().copied().min_by_key(|byte_index| {
        let actual_line = line_number_at_byte_index(content, *byte_index);
        let distance_to_start = actual_line.abs_diff(expected);
        let distance_to_end = actual_line.abs_diff(expected.saturating_add(old_line_count - 1));
        distance_to_start.min(distance_to_end)
    })
}

fn line_number_at_byte_index(content: &str, byte_index: usize) -> usize {
    content[..byte_index.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextLineEndings {
    Lf,
    Crlf,
}

fn detect_text_line_endings(content: &str) -> TextLineEndings {
    let crlf_count = content.match_indices("\r\n").count();
    let lf_count = content.bytes().filter(|byte| *byte == b'\n').count();
    if crlf_count > lf_count.saturating_sub(crlf_count) {
        TextLineEndings::Crlf
    } else {
        TextLineEndings::Lf
    }
}

fn normalize_to_lf(content: &str) -> String {
    content.replace("\r\n", "\n")
}

fn restore_line_endings(content: &str, line_endings: TextLineEndings) -> String {
    match line_endings {
        TextLineEndings::Lf => content.to_string(),
        TextLineEndings::Crlf => content.replace('\n', "\r\n"),
    }
}

fn collect_edit_warnings(path: &Path, old_content: &str, new_content: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(extension) = path.extension().and_then(OsStr::to_str) else {
        return warnings;
    };
    let extension = extension.to_ascii_lowercase();
    let is_js_like = matches!(extension.as_str(), "js" | "jsx" | "ts" | "tsx");
    let is_checked_code = is_js_like || extension == "rs";

    if is_js_like && new_content.contains("\\${") {
        warnings.push(format!(
            "{} contains `\\${{`; TS/JS template expressions usually require `${{...}}` without the backslash.",
            path.to_string_lossy()
        ));
    }

    if is_checked_code {
        let old_issues = bracket_balance_issues(old_content, is_js_like);
        let new_issues = bracket_balance_issues(new_content, is_js_like);
        if !new_issues.is_empty() && old_issues != new_issues {
            warnings.push(format!(
                "{} may have unbalanced delimiters after edit: {}",
                path.to_string_lossy(),
                new_issues.join("; ")
            ));
        }
    }

    warnings
}

fn bracket_balance_issues(content: &str, js_like: bool) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ScanState {
        Code,
        LineComment,
        BlockComment,
        DoubleString,
        SingleString,
        TemplateString,
    }

    let mut state = ScanState::Code;
    let mut escaped = false;
    let mut stack = Vec::<(char, usize, usize)>::new();
    let mut issues = Vec::<String>::new();
    let mut chars = content.chars().peekable();
    let mut line = 1usize;
    let mut column = 0usize;

    while let Some(ch) = chars.next() {
        column += 1;
        let next = chars.peek().copied();

        match state {
            ScanState::Code => match ch {
                '/' if next == Some('/') => {
                    chars.next();
                    column += 1;
                    state = ScanState::LineComment;
                }
                '/' if next == Some('*') => {
                    chars.next();
                    column += 1;
                    state = ScanState::BlockComment;
                }
                '"' => {
                    escaped = false;
                    state = ScanState::DoubleString;
                }
                '\'' if js_like => {
                    escaped = false;
                    state = ScanState::SingleString;
                }
                '`' if js_like => {
                    escaped = false;
                    state = ScanState::TemplateString;
                }
                '(' | '[' | '{' => stack.push((ch, line, column)),
                ')' | ']' | '}' => {
                    let expected = matching_open_delimiter(ch);
                    match stack.pop() {
                        Some((open, _, _)) if open == expected => {}
                        Some((open, open_line, open_column)) => issues.push(format!(
                            "line {line}, column {column}: found `{ch}` while `{}` opened at line {open_line}, column {open_column}",
                            open
                        )),
                        None => issues.push(format!(
                            "line {line}, column {column}: unmatched closing `{ch}`"
                        )),
                    }
                }
                _ => {}
            },
            ScanState::LineComment => {
                if ch == '\n' {
                    state = ScanState::Code;
                }
            }
            ScanState::BlockComment => {
                if ch == '*' && next == Some('/') {
                    chars.next();
                    column += 1;
                    state = ScanState::Code;
                }
            }
            ScanState::DoubleString => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    state = ScanState::Code;
                }
            }
            ScanState::SingleString => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '\'' {
                    state = ScanState::Code;
                }
            }
            ScanState::TemplateString => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '`' {
                    state = ScanState::Code;
                }
            }
        }

        if ch == '\n' {
            line += 1;
            column = 0;
            if state == ScanState::LineComment {
                state = ScanState::Code;
            }
        }
    }

    issues.extend(
        stack
            .into_iter()
            .rev()
            .map(|(open, open_line, open_column)| {
                format!("line {open_line}, column {open_column}: unclosed `{open}`")
            }),
    );
    issues.truncate(5);
    issues
}

fn matching_open_delimiter(close: char) -> char {
    match close {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        other => other,
    }
}

fn multi_edit_preview(operations: &[MultiEditFileOperation]) -> String {
    let changes = operations
        .iter()
        .enumerate()
        .map(|(index, operation)| match operation {
            MultiEditFileOperation::Edit { path, edits } => {
                format!("{}. edit {} edits={}", index + 1, path, edits.len())
            }
            MultiEditFileOperation::Create {
                path,
                content,
                overwrite,
            } => format!(
                "{}. create {} bytes={} overwrite={}",
                index + 1,
                path,
                content.len(),
                overwrite
            ),
            MultiEditFileOperation::Delete { path } => {
                format!("{}. delete {}", index + 1, path)
            }
            MultiEditFileOperation::Move {
                from,
                to,
                overwrite,
            } => format!(
                "{}. move {} -> {} overwrite={}",
                index + 1,
                from,
                to,
                overwrite
            ),
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("multi_edit_file\n{}", changes)
}

fn multi_edit_preview_changes(operations: &[MultiEditFileOperation]) -> Value {
    json!(operations
        .iter()
        .map(|operation| match operation {
            MultiEditFileOperation::Edit { path, edits } => json!({
                "kind": "edit",
                "path": path,
                "edits": edits.iter().map(|edit| json!({
                    "oldBytes": edit.old_string.len(),
                    "newBytes": edit.new_string.len(),
                    "replaceAll": edit.replace_all,
                    "startLine": edit.start_line
                })).collect::<Vec<_>>()
            }),
            MultiEditFileOperation::Create {
                path,
                content,
                overwrite,
            } => json!({
                "kind": "create",
                "path": path,
                "contentBytes": content.len(),
                "overwrite": overwrite
            }),
            MultiEditFileOperation::Delete { path } => json!({
                "kind": "delete",
                "path": path
            }),
            MultiEditFileOperation::Move {
                from,
                to,
                overwrite,
            } => json!({
                "kind": "move",
                "from": from,
                "to": to,
                "overwrite": overwrite
            }),
        })
        .collect::<Vec<_>>())
}

fn file_edit_summary_text(summary: &FileEditSummary) -> String {
    let mut lines = vec!["Success. Updated the following files:".to_string()];
    for path in &summary.added {
        lines.push(format!("A {path}"));
    }
    for path in &summary.modified {
        lines.push(format!("M {path}"));
    }
    for path in &summary.deleted {
        lines.push(format!("D {path}"));
    }
    for path in &summary.moved {
        lines.push(format!("R {path}"));
    }
    format!("{}\n", lines.join("\n"))
}

