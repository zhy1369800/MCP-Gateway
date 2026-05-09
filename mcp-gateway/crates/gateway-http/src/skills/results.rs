fn decode_tool_args<T>(value: &Value) -> Result<T, AppError>
where
    T: for<'de> Deserialize<'de>,
{
    let payload = if value.is_null() {
        json!({})
    } else {
        value.clone()
    };
    serde_json::from_value(payload)
        .map_err(|error| AppError::BadRequest(format!("invalid tool arguments: {error}")))
}

fn tool_success(text: String, structured: Value) -> ToolResult {
    ToolResult {
        text,
        structured,
        is_error: false,
    }
}

fn tool_success_with_planning_reminder(
    text: String,
    mut structured: Value,
    hints: PlanningSuccessHints,
) -> ToolResult {
    if hints.planning_reminder.is_none() && hints.shell_command_reminder.is_none() {
        return tool_success(text, structured);
    };

    if let Value::Object(fields) = &mut structured {
        if let Some(planning_reminder) = hints.planning_reminder {
            fields.insert(
                "planningReminder".to_string(),
                Value::String(planning_reminder),
            );
        }
        if let Some(shell_command_reminder) = hints.shell_command_reminder {
            fields.insert(
                "shellCommandReminder".to_string(),
                Value::String(shell_command_reminder),
            );
        }
    }

    tool_success(text, structured)
}

fn tool_error(text: String, mut structured: Value) -> ToolResult {
    normalize_tool_error_structured_content(&text, &mut structured);
    ToolResult {
        text,
        structured,
        is_error: true,
    }
}

fn normalize_tool_error_structured_content(text: &str, structured: &mut Value) {
    let Value::Object(fields) = structured else {
        return;
    };

    let status = fields
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("error")
        .to_string();
    fields
        .entry("status".to_string())
        .or_insert_with(|| Value::String("error".to_string()));
    fields
        .entry("code".to_string())
        .or_insert_with(|| Value::String(default_tool_error_code(&status).to_string()));
    fields
        .entry("message".to_string())
        .or_insert_with(|| Value::String(text.to_string()));
}

fn default_tool_error_code(status: &str) -> &'static str {
    match status {
        "blocked" => "PolicyBlocked",
        "rejected" => "ConfirmationRejected",
        "timeout" => "ConfirmationTimeout",
        "failed" => "ToolFailed",
        _ => "ToolError",
    }
}

fn skill_debug_metadata_enabled() -> bool {
    env::var("MCP_GATEWAY_SKILL_DEBUG_METADATA")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

fn tool_error_with_edit_failure_reminder(
    text: String,
    mut structured: Value,
    edit_failure_reminder: Option<String>,
) -> ToolResult {
    if let Some(edit_failure_reminder) = edit_failure_reminder {
        if let Value::Object(fields) = &mut structured {
            fields.insert(
                "editFailureReminder".to_string(),
                Value::String(edit_failure_reminder),
            );
        }
    }

    tool_error(text, structured)
}

fn planning_scope_key(session_id: Option<&str>) -> String {
    session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("session:{value}"))
        .unwrap_or_else(|| "session:default".to_string())
}

fn planning_scope_prefix(scope: &str) -> String {
    format!("{scope}::planning::")
}

fn planning_state_key(scope: &str, planning_id: &str) -> String {
    format!("{}{}", planning_scope_prefix(scope), planning_id)
}

fn resolve_planning_state_key(
    states: &HashMap<String, PlanningState>,
    scope: &str,
    planning_id: &str,
) -> Result<String, PlanningLookupError> {
    let exact_key = planning_state_key(scope, planning_id);
    if states.contains_key(&exact_key) {
        return Ok(exact_key);
    }

    let matches: Vec<&String> = states
        .iter()
        .filter_map(|(key, state)| {
            if state.planning_id == planning_id {
                Some(key)
            } else {
                None
            }
        })
        .collect();

    match matches.as_slice() {
        [] => Err(PlanningLookupError::Unknown),
        [key] => Ok((*key).clone()),
        _ => Err(PlanningLookupError::Ambiguous),
    }
}

fn planning_id_for_plan(plan: &[PlanItem]) -> String {
    let mut hasher = DefaultHasher::new();
    "task-planning-v2".hash(&mut hasher);
    for item in plan {
        item.step.trim().hash(&mut hasher);
    }
    format!("plan-{:016x}", hasher.finish())
}

fn plan_all_completed(plan: &[PlanItem]) -> bool {
    !plan.is_empty()
        && plan
            .iter()
            .all(|item| item.status == PlanItemStatus::Completed)
}

fn current_plan_item(plan: &[PlanItem]) -> Option<(usize, &PlanItem)> {
    plan.iter()
        .enumerate()
        .find(|(_, item)| item.status == PlanItemStatus::InProgress)
        .map(|(idx, item)| (idx + 1, item))
}

fn next_pending_item_index(plan: &[PlanItem], after_index: usize) -> Option<usize> {
    plan.iter()
        .enumerate()
        .skip(after_index.saturating_add(1))
        .find(|(_, item)| item.status == PlanItemStatus::Pending)
        .map(|(idx, _)| idx)
        .or_else(|| {
            plan.iter()
                .enumerate()
                .find(|(_, item)| item.status == PlanItemStatus::Pending)
                .map(|(idx, _)| idx)
        })
}

fn shell_command_starts_with_rg(command: &str) -> bool {
    let trimmed = command.trim_start();
    trimmed == "rg"
        || trimmed.starts_with("rg ")
        || trimmed.starts_with("rg\t")
        || trimmed == "rg.exe"
        || trimmed.starts_with("rg.exe ")
        || trimmed.starts_with("rg.exe\t")
}

fn validate_plan_items(plan: &[PlanItem]) -> Result<(), AppError> {
    if plan.is_empty() {
        return Err(AppError::BadRequest(
            "task-planning update requires at least one plan item".to_string(),
        ));
    }
    if plan.len() > 50 {
        return Err(AppError::BadRequest(
            "task-planning plan cannot contain more than 50 items".to_string(),
        ));
    }

    let mut in_progress = 0usize;
    for (idx, item) in plan.iter().enumerate() {
        if item.step.trim().is_empty() {
            return Err(AppError::BadRequest(format!(
                "task-planning plan item {} has an empty step",
                idx + 1
            )));
        }
        if item.step.chars().count() > 500 {
            return Err(AppError::BadRequest(format!(
                "task-planning plan item {} is too long; keep steps under 500 characters",
                idx + 1
            )));
        }
        if item.status == PlanItemStatus::InProgress {
            in_progress += 1;
        }
    }
    if in_progress > 1 {
        return Err(AppError::BadRequest(
            "task-planning plan can have at most one in_progress item".to_string(),
        ));
    }
    Ok(())
}

fn planning_gate_error(
    tool: BuiltinTool,
    reason: &str,
    next_step: &str,
    planning_id: Option<&str>,
) -> ToolResult {
    tool_error(
        format!(
            "{} requires an active task-planning state before real builtin tool calls. {next_step}",
            tool.name()
        ),
        json!({
            "status": "blocked",
            "tool": tool.name(),
            "reason": reason,
            "planningRequired": true,
            "planningId": planning_id,
            "nextStep": next_step
        }),
    )
}

fn planning_gate_instructions() -> &'static str {
    "## Planning Gate\n\nWhen the bundled `task-planning` skill is enabled, real builtin tool calls are gated by an active plan. Before using this skill for any non-documentation action, call `task-planning` with `action: \"update\"` and a concise todo list. The gateway returns a content-derived `planningId`. Pass it as `planningId` on subsequent builtin tool calls. Reuse the same planningId across multiple tool calls while working through the plan. Successful builtin tool results may include a single `planningReminder` for the current `in_progress` item. If that item is done, use `task-planning` with `action: \"set_status\"`, the active `planningId`, and `status: \"completed\"`; do not resend the full plan for simple status changes. Use full `update` only when the plan steps or approach change. When all plan items are updated to `completed`, that planningId is closed and can no longer be used for tool calls. Documentation reads of SKILL.md files do not require planning fields."
}

fn confirmation_rejected_result(
    tool_name: &str,
    confirmation_id: &str,
    timed_out: bool,
) -> ToolResult {
    let text = if timed_out {
        "MCP Gateway 已拒绝此命令：确认请求 60 秒内未被批准，命令没有执行。要执行该命令，请重新提交并在 Pending Confirmations 中批准；或者把匹配的 skills.policy 规则改为 allow，让它默认运行。"
    } else {
        "MCP Gateway 已拒绝此命令：用户拒绝了确认请求，命令没有执行。要执行该命令，请重新提交并在 Pending Confirmations 中批准；或者把匹配的 skills.policy 规则改为 allow，让它默认运行。"
    };
    let reason = if timed_out {
        "timeout"
    } else {
        "user_rejected"
    };
    let status = if timed_out { "timeout" } else { "rejected" };
    tool_error(
        text.to_string(),
        json!({
            "status": status,
            "tool": tool_name,
            "reason": reason,
            "confirmationId": confirmation_id
        }),
    )
}

fn mcp_gateway_policy_denied_text(reason: &str) -> String {
    format!(
        "MCP Gateway 已拒绝此命令：该命令命中了当前网关策略中的拒绝规则（deny）或默认拒绝动作，因此没有执行。匹配原因：{reason}。如果你确认此类命令应该可以运行，请在可视化规则管理中把匹配规则从“拒绝/deny”改为“用户确认/confirm”，让它在执行前请求用户批准；也可以删除或禁用这条拒绝规则，让后续规则或默认动作接管。"
    )
}

fn mcp_gateway_policy_denied_help() -> Value {
    json!({
        "message": "This command was blocked by MCP Gateway policy and was not executed.",
        "uiHint": "Open visual policy rule management, find the matching deny rule, then change it to confirm or remove/disable it.",
        "suggestedActions": [
            "change_matching_rule_to_confirm",
            "remove_or_disable_matching_deny_rule"
        ]
    })
}

fn error_to_tool_result(error: AppError) -> ToolResult {
    let code = format!("{:?}", error.code());
    let message = error.message();
    let structured = json!({
        "status": "error",
        "code": code,
        "message": message.clone()
    });
    ToolResult {
        text: message,
        structured,
        is_error: matches!(
            error.code(),
            ErrorCode::BadRequest
                | ErrorCode::ValidationFailed
                | ErrorCode::NotFound
                | ErrorCode::Conflict
                | ErrorCode::UpstreamFailed
                | ErrorCode::Internal
                | ErrorCode::Unauthorized
        ),
    }
}

fn command_output_text(stdout: &str, stderr: &str) -> String {
    let stdout = stdout.trim_end();
    let stderr = stderr.trim_end();
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}\n\n[stderr]\n{stderr}"),
        (true, true) => "command completed with no output".to_string(),
    }
}

fn command_failure_text(exit_code: i32, stdout: &str, stderr: &str) -> String {
    let output = command_output_text(stdout, stderr);
    if output == "command completed with no output" {
        format!("command finished with non-zero exit code ({exit_code}) and no output")
    } else {
        format!("command finished with non-zero exit code ({exit_code}).\n{output}")
    }
}

fn command_timeout_text(timeout_ms: u64, stdout: &str, stderr: &str) -> String {
    let output = command_output_text(stdout, stderr);
    if output == "command completed with no output" {
        format!("command timed out after {timeout_ms}ms and produced no output")
    } else {
        format!("command timed out after {timeout_ms}ms.\nLast output:\n{output}")
    }
}

fn summarize_discovered_skills(skills: &[DiscoveredSkill]) -> Vec<SkillSummary> {
    skills
        .iter()
        .map(|skill| SkillSummary {
            skill: skill.skill.clone(),
            description: skill.description.clone(),
            root: normalize_display_path(&skill.root),
            path: normalize_display_path(&skill.path),
            has_scripts: skill.has_scripts,
        })
        .collect()
}

fn summarize_builtin_skills(cfg: &BuiltinToolsConfig) -> Vec<SkillSummary> {
    builtin_tools(cfg)
        .into_iter()
        .map(|tool| {
            let frontmatter = builtin_skill_frontmatter(tool);
            SkillSummary {
                skill: tool.name().to_string(),
                description: frontmatter.description,
                root: builtin_skills_root_uri().to_string(),
                path: builtin_skill_uri_root(tool),
                has_scripts: false,
            }
        })
        .collect()
}

