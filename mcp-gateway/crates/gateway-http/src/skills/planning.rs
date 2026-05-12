impl SkillsService {
    fn planning_enabled(config: &GatewayConfig) -> bool {
        config.skills.builtin_tools.task_planning
    }

    async fn check_planning_gate(
        &self,
        config: &GatewayConfig,
        planning_scope: &str,
        tool: BuiltinTool,
        planning_id: Option<&str>,
    ) -> Option<ToolResult> {
        if !Self::planning_enabled(config) || tool == BuiltinTool::TaskPlanning {
            return None;
        }

        let Some(planning_id) = planning_id.map(str::trim).filter(|value| !value.is_empty()) else {
            return Some(planning_gate_error(
                tool,
                "missing planningId",
                "Call task-planning with action=\"update\" first, then retry this tool with the returned planningId.",
                None,
            ));
        };

        let guard = self.planning.read().await;
        let key = match resolve_planning_state_key(&guard, planning_scope, planning_id) {
            Ok(key) => key,
            Err(PlanningLookupError::Unknown) => {
                return Some(planning_gate_error(
                    tool,
                    "unknown planningId",
                    "The supplied planningId is not active. This can happen after all plan items are completed, task-planning clear, or a gateway restart. Call task-planning update again and use the returned planningId.",
                    Some(planning_id),
                ));
            }
            Err(PlanningLookupError::Ambiguous) => {
                return Some(planning_gate_error(
                    tool,
                    "ambiguous planningId",
                    "The supplied planningId exists in more than one client/session scope. Call task-planning update again for this request and use the returned planningId.",
                    Some(planning_id),
                ));
            }
        };
        let Some(state) = guard.get(&key) else {
            return Some(planning_gate_error(
                tool,
                "unknown planningId",
                "The supplied planningId is not active. Call task-planning update again and use the returned planningId.",
                Some(planning_id),
            ));
        };
        debug_assert_eq!(state.planning_id, planning_id);
        let _active_plan_len = state.plan.len();
        let _active_explanation = state.explanation.as_deref();
        let _active_updated_at = state.updated_at;

        None
    }

    async fn planning_success_hints(
        &self,
        config: &GatewayConfig,
        planning_scope: &str,
        planning_id: Option<&str>,
        tool: BuiltinTool,
        shell_command: Option<&str>,
    ) -> PlanningSuccessHints {
        if !Self::planning_enabled(config) {
            return PlanningSuccessHints::default();
        }
        let Some(planning_id) = planning_id.map(str::trim).filter(|value| !value.is_empty()) else {
            return PlanningSuccessHints::default();
        };
        let guard = self.planning.read().await;
        let Ok(key) = resolve_planning_state_key(&guard, planning_scope, planning_id) else {
            return PlanningSuccessHints::default();
        };
        drop(guard);

        let mut guard = self.planning.write().await;
        let Some(state) = guard.get_mut(&key) else {
            return PlanningSuccessHints::default();
        };

        state.consecutive_multi_edit_file_failures = 0;
        let shell_command_reminder = if tool == BuiltinTool::ShellCommand {
            if shell_command.is_some_and(shell_command_starts_with_rg) {
                state.consecutive_shell_commands = 0;
                None
            } else {
                state.consecutive_shell_commands =
                    state.consecutive_shell_commands.saturating_add(1);
                if state.consecutive_shell_commands >= 3 {
                    Some(format!(
                    "This planningId has used shell_command {} times in a row without a plan update. Consider combining related commands into one shell call, reading multiple files in one pass when useful, and preferring efficient search commands such as `rg` and `rg --files` when exploring the codebase.",
                    state.consecutive_shell_commands
                ))
                } else {
                    None
                }
            }
        } else {
            state.consecutive_shell_commands = 0;
            None
        };

        let planning_reminder = if let Some((idx, item)) = current_plan_item(&state.plan) {
            let next_item = state
                .plan
                .iter()
                .enumerate()
                .skip(idx)
                .find(|(_, item)| item.status == PlanItemStatus::Pending)
                .map(|(next_idx, next_item)| (next_idx + 1, next_item));
            let mut reminder = format!(
                "Current plan item #{idx} is in_progress: \"{}\".",
                item.step
            );
            if let Some((next_idx, next_item)) = next_item {
                reminder.push_str(&format!(
                    " Next plan item #{next_idx}: \"{}\".",
                    next_item.step
                ));
                reminder.push_str(&format!(
                    " Remember to decide whether the current plan item is complete and whether to start the next plan item. If complete, call task-planning with {{\"action\":\"set_status\",\"planningId\":\"{}\",\"item\":{idx},\"status\":\"completed\"}}; the gateway will start the next pending item when appropriate.",
                    state.planning_id
                ));
            } else {
                reminder.push_str(&format!(
                    " Remember to decide whether this final plan item is complete and whether the whole plan is done. If complete, call task-planning with {{\"action\":\"set_status\",\"planningId\":\"{}\",\"item\":{idx},\"status\":\"completed\"}}.",
                    state.planning_id
                ));
            }
            Some(reminder)
        } else {
            Some(format!(
                "No plan item is currently in_progress for planningId {}. If work moved to a specific item, call task-planning with {{\"action\":\"set_status\",\"planningId\":\"{}\",\"item\":<1-based item number>,\"status\":\"in_progress\"}}.",
                state.planning_id, state.planning_id
            ))
        };

        PlanningSuccessHints {
            planning_reminder,
            shell_command_reminder,
        }
    }

    async fn planning_edit_failure_reminder(
        &self,
        config: &GatewayConfig,
        planning_scope: &str,
        planning_id: Option<&str>,
        tool: BuiltinTool,
    ) -> Option<String> {
        if !Self::planning_enabled(config) {
            return None;
        }
        let planning_id = planning_id
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let guard = self.planning.read().await;
        let key = resolve_planning_state_key(&guard, planning_scope, planning_id).ok()?;
        drop(guard);

        let mut guard = self.planning.write().await;
        let state = guard.get_mut(&key)?;
        state.consecutive_shell_commands = 0;

        if tool != BuiltinTool::MultiEditFile {
            return None;
        }

        state.consecutive_multi_edit_file_failures =
            state.consecutive_multi_edit_file_failures.saturating_add(1);
        let tool_name = BuiltinTool::MultiEditFile.name();
        let count = state.consecutive_multi_edit_file_failures;

        if count >= 3 {
            Some(format!(
                "{tool_name} has failed {count} times in a row for this planningId. Consider simplifying the edit operation, inspecting the exact file content first, splitting unrelated changes, or using shell_command with a focused script when structured edits keep failing."
            ))
        } else {
            None
        }
    }

}
