fn task_planning_tool_definition(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Value {
    json!({
            "name": BuiltinTool::TaskPlanning.name(),
            "description": render_builtin_tool_description(BuiltinTool::TaskPlanning, os, now, cfg.task_planning, cfg.read_file),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": [],
                "properties": {
                    "exec": {
                        "type": "string",
                        "description": "Documentation read command. First call may read the complete builtin://task-planning/SKILL.md without skillToken."
                    },
                    "action": {
                        "type": "string",
                        "enum": ["update", "set_status", "clear"],
                        "description": "Use update to create or replace the full plan, set_status for concise item status changes, and clear to remove active plan state."
                    },
                    "explanation": {
                        "type": "string",
                        "description": "Optional short reason for this plan update."
                    },
                    "plan": {
                        "type": "array",
                        "description": "Required for action=update. Concise todo list for the current task.",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["step", "status"],
                            "properties": {
                                "step": {
                                    "type": "string",
                                    "description": "One task step."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "Step state. At most one item may be in_progress."
                                }
                            }
                        }
                    },
                    "planningId": {
                        "type": "string",
                        "description": "Required for action=set_status. Optional for action=clear. If omitted for clear, all active plans for this client/session are cleared."
                    },
                    "item": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional 1-based plan item number for action=set_status. If omitted, the current in_progress item is updated."
                    },
                    "status": {
                        "type": "string",
                        "enum": ["pending", "in_progress", "completed"],
                        "description": "Required for action=set_status. New state for the selected item."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for action=update, action=set_status, or action=clear. First read the complete builtin://task-planning/SKILL.md without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Documentation reads do not require it."
                    }
                }
            }
    })
}

impl SkillsService {
    async fn handle_builtin_task_planning(
        &self,
        args: TaskPlanningArgs,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
        if let Some(exec) = args.exec.as_deref() {
            let command_preview = exec.trim().to_string();
            if command_preview.is_empty() {
                return Err(AppError::BadRequest("exec cannot be empty".to_string()));
            }

            if let Some((tool, matched_path)) = builtin_skill_doc_read(&command_preview) {
                return Ok(builtin_skill_doc_result(
                    tool,
                    &command_preview,
                    matched_path,
                    builtin_skill_token(tool),
                    true,
                ));
            }

            if args.action.is_none() {
                return Err(AppError::BadRequest(
                    "task-planning exec is only for reading SKILL.md; use action=\"update\", action=\"set_status\", or action=\"clear\" for plan state"
                        .to_string(),
                ));
            }
        }

        if let Some(result) = validate_skill_token_result(
            BuiltinTool::TaskPlanning.name(),
            &builtin_skill_token(BuiltinTool::TaskPlanning),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        match args.action.unwrap_or(TaskPlanningAction::Update) {
            TaskPlanningAction::Update => {
                validate_plan_items(&args.plan)?;
                let planning_id = planning_id_for_plan(&args.plan);
                let key = planning_state_key(planning_scope, &planning_id);
                let now = Utc::now();
                let mut guard = self.planning.write().await;
                let all_completed = plan_all_completed(&args.plan);
                if all_completed {
                    guard.remove(&key);
                    return Ok(tool_success(
                        "Plan completed".to_string(),
                        json!({
                            "status": "completed",
                            "tool": BuiltinTool::TaskPlanning.name(),
                            "planning": {
                                "active": false,
                                "completed": true,
                                "planningId": planning_id,
                                "updatedAt": now,
                                "explanation": args.explanation,
                                "plan": args.plan
                            }
                        }),
                    ));
                }
                guard.insert(
                    key,
                    PlanningState {
                        planning_id: planning_id.clone(),
                        plan: args.plan.clone(),
                        explanation: args.explanation.clone(),
                        consecutive_shell_commands: 0,
                        consecutive_multi_edit_file_failures: 0,
                        consecutive_read_file_failures: 0,
                        consecutive_chrome_cdp_failures: 0,
                        officecli_pending_wps_cleanup: None,
                        updated_at: now,
                    },
                );
                Ok(tool_success(
                    "Plan updated".to_string(),
                    json!({
                        "status": "completed",
                        "tool": BuiltinTool::TaskPlanning.name(),
                        "planning": {
                            "active": true,
                            "planningId": planning_id,
                            "needsUpdate": false,
                            "updatedAt": now,
                            "explanation": args.explanation,
                            "plan": args.plan
                        }
                    }),
                ))
            }
            TaskPlanningAction::SetStatus => {
                let planning_id = args
                    .planning_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        AppError::BadRequest(
                            "task-planning set_status requires planningId".to_string(),
                        )
                    })?;
                let status = args.status.clone().ok_or_else(|| {
                    AppError::BadRequest("task-planning set_status requires status".to_string())
                })?;
                let now = Utc::now();
                let mut guard = self.planning.write().await;
                let key = resolve_planning_state_key(&guard, planning_scope, planning_id).map_err(
                    |error| match error {
                        PlanningLookupError::Unknown => AppError::BadRequest(format!(
                            "task-planning planningId is not active: {planning_id}"
                        )),
                        PlanningLookupError::Ambiguous => AppError::BadRequest(format!(
                            "task-planning planningId is active in more than one client/session scope: {planning_id}"
                        )),
                    },
                )?;
                let state = guard.get_mut(&key).ok_or_else(|| {
                    AppError::BadRequest(format!(
                        "task-planning planningId is not active: {planning_id}"
                    ))
                })?;
                state.consecutive_shell_commands = 0;
                state.consecutive_multi_edit_file_failures = 0;
                state.consecutive_read_file_failures = 0;
                state.consecutive_chrome_cdp_failures = 0;
                state.officecli_pending_wps_cleanup = None;
                let item_index = match args.item {
                    Some(item) if item == 0 || item > state.plan.len() => {
                        return Err(AppError::BadRequest(format!(
                            "task-planning item must be between 1 and {}",
                            state.plan.len()
                        )));
                    }
                    Some(item) => item - 1,
                    None => state
                        .plan
                        .iter()
                        .position(|item| item.status == PlanItemStatus::InProgress)
                        .ok_or_else(|| {
                            AppError::BadRequest(
                                "task-planning set_status without item requires one in_progress plan item"
                                    .to_string(),
                            )
                        })?,
                };
                state.plan[item_index].status = status;
                let auto_started_item = if state.plan[item_index].status
                    == PlanItemStatus::Completed
                    && current_plan_item(&state.plan).is_none()
                {
                    next_pending_item_index(&state.plan, item_index).map(|next_index| {
                        state.plan[next_index].status = PlanItemStatus::InProgress;
                        (next_index, state.plan[next_index].step.clone())
                    })
                } else {
                    None
                };
                validate_plan_items(&state.plan)?;
                state.updated_at = now;
                let plan = state.plan.clone();
                let explanation = state.explanation.clone();
                let step = plan[item_index].step.clone();
                let updated_status = plan[item_index].status.clone();
                if plan_all_completed(&plan) {
                    guard.remove(&key);
                    return Ok(tool_success(
                        "Plan completed".to_string(),
                        json!({
                            "status": "completed",
                            "tool": BuiltinTool::TaskPlanning.name(),
                            "planning": {
                                "active": false,
                                "completed": true,
                                "planningId": planning_id,
                                "updatedAt": now,
                                "updatedItem": {
                                    "item": item_index + 1,
                                    "step": step,
                                    "status": updated_status
                                },
                                "explanation": explanation,
                                "plan": plan
                            }
                        }),
                    ));
                }

                let text = if let Some((next_index, next_step)) = auto_started_item.as_ref() {
                    format!(
                        "Plan item updated; start plan item #{}: {}",
                        next_index + 1,
                        next_step
                    )
                } else {
                    "Plan item updated".to_string()
                };
                let next_item = auto_started_item.map(|(next_index, next_step)| {
                    json!({
                        "item": next_index + 1,
                        "step": next_step,
                        "status": PlanItemStatus::InProgress
                    })
                });
                Ok(tool_success(
                    text,
                    json!({
                        "status": "completed",
                        "tool": BuiltinTool::TaskPlanning.name(),
                        "planning": {
                            "active": true,
                            "planningId": planning_id,
                            "updatedAt": now,
                            "updatedItem": {
                                "item": item_index + 1,
                                "step": step,
                                "status": updated_status
                            },
                            "nextItem": next_item,
                            "explanation": explanation,
                            "plan": plan
                        }
                    }),
                ))
            }
            TaskPlanningAction::Clear => {
                let mut guard = self.planning.write().await;
                let removed = if let Some(planning_id) = args
                    .planning_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    match resolve_planning_state_key(&guard, planning_scope, planning_id) {
                        Ok(key) => guard.remove(&key).is_some(),
                        Err(PlanningLookupError::Unknown) => false,
                        Err(PlanningLookupError::Ambiguous) => {
                            return Err(AppError::BadRequest(format!(
                                "task-planning planningId is active in more than one client/session scope: {planning_id}"
                            )));
                        }
                    }
                } else {
                    let prefix = planning_scope_prefix(planning_scope);
                    let before = guard.len();
                    guard.retain(|key, _| !key.starts_with(&prefix));
                    guard.len() != before
                };

                Ok(tool_success(
                    "Plan cleared".to_string(),
                    json!({
                        "status": "completed",
                        "tool": BuiltinTool::TaskPlanning.name(),
                        "planning": {
                            "active": false,
                            "removed": removed,
                            "planningId": args.planning_id
                        }
                    }),
                ))
            }
        }
    }
}
