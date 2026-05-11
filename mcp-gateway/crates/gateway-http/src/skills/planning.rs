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
                state.consecutive_read_file_failures = 0;
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

        if tool == BuiltinTool::ReadFile {
            state.consecutive_read_file_failures = 0;
        }

        if tool == BuiltinTool::ChromeCdp {
            if shell_command
                .map(cdp_command_resets_stuck_counter)
                .unwrap_or(false)
            {
                state.consecutive_chrome_cdp_failures = 0;
            }
        }

        let office_cli_post_create_reminder = if tool == BuiltinTool::OfficeCli {
            let command = shell_command.unwrap_or("");
            if let Some(created_file) = parse_officecli_create_file(command) {
                state.officecli_pending_wps_cleanup = Some(OfficeCliPendingCleanup {
                    file: created_file,
                    created_at: Instant::now(),
                });
                None
            } else if officecli_command_clears_wps(command, state.officecli_pending_wps_cleanup.as_ref()) {
                state.officecli_pending_wps_cleanup = None;
                None
            } else {
                office_cli_pending_reminder(&mut state.officecli_pending_wps_cleanup, tool)
            }
        } else {
            office_cli_pending_reminder(&mut state.officecli_pending_wps_cleanup, tool)
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
            read_failure_reminder: None,
            cdp_stuck_reminder: None,
            office_cli_post_create_reminder,
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

    async fn planning_read_failure_reminder(
        &self,
        config: &GatewayConfig,
        planning_scope: &str,
        planning_id: Option<&str>,
        kind: ReadFailureKind,
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
        if !matches!(
            kind,
            ReadFailureKind::NotFound | ReadFailureKind::Binary | ReadFailureKind::TooLarge
        ) {
            return None;
        }
        state.consecutive_read_file_failures =
            state.consecutive_read_file_failures.saturating_add(1);
        let count = state.consecutive_read_file_failures;
        if count >= 3 {
            Some(format!(
                "read_file has failed {count} times in a row for this planningId (last cause: {}). Consider locating the target with `shell_command` + `rg` or `rg --files` first, then reading a bounded window with offset/limit; for binary artifacts drop back to `shell_command` with a focused probe instead of read_file.",
                kind.as_str()
            ))
        } else {
            None
        }
    }

    async fn planning_cdp_failure_reminder(
        &self,
        config: &GatewayConfig,
        planning_scope: &str,
        planning_id: Option<&str>,
        stdout: &str,
        stderr: &str,
    ) -> Option<String> {
        if !Self::planning_enabled(config) {
            return None;
        }
        let planning_id = planning_id
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        if !cdp_output_indicates_stuck(stdout, stderr) {
            return None;
        }
        let guard = self.planning.read().await;
        let key = resolve_planning_state_key(&guard, planning_scope, planning_id).ok()?;
        drop(guard);

        let mut guard = self.planning.write().await;
        let state = guard.get_mut(&key)?;
        state.consecutive_chrome_cdp_failures =
            state.consecutive_chrome_cdp_failures.saturating_add(1);
        let count = state.consecutive_chrome_cdp_failures;
        if count >= 3 {
            Some(format!(
                "chrome-cdp has looked stuck {count} times in a row for this planningId (target/WebSocket/timeout signals in the last failure). Run `stop`, then re-run `open <url>` or `launch <url>` to recreate the managed browser. Do not ask the user to close their own Chrome or disable their DevTools debugger port."
            ))
        } else {
            None
        }
    }

}

#[derive(Debug, Clone, Copy)]
enum ReadFailureKind {
    NotFound,
    Binary,
    TooLarge,
    Other,
}

impl ReadFailureKind {
    fn classify(message: &str) -> ReadFailureKind {
        let lower = message.to_ascii_lowercase();
        if lower.contains("does not exist") || lower.contains("no such file") {
            ReadFailureKind::NotFound
        } else if lower.contains("appears to be binary") {
            ReadFailureKind::Binary
        } else if lower.contains("too large") {
            ReadFailureKind::TooLarge
        } else {
            ReadFailureKind::Other
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            ReadFailureKind::NotFound => "not-found",
            ReadFailureKind::Binary => "binary",
            ReadFailureKind::TooLarge => "too-large",
            ReadFailureKind::Other => "other",
        }
    }
}

const OFFICECLI_WPS_TTL_SECONDS: u64 = 120;

fn parse_officecli_create_file(command: &str) -> Option<String> {
    let tokens = split_shell_tokens(command);
    let mut iter = tokens.iter().map(String::as_str);
    let program = iter.next()?;
    if !matches!(program.to_ascii_lowercase().as_str(), "officecli" | "officecli.exe") {
        return None;
    }
    match iter.next() {
        Some("create") => {}
        _ => return None,
    }
    let file = iter.next()?;
    if file.starts_with('-') {
        return None;
    }
    Some(file.to_string())
}

fn officecli_command_clears_wps(
    command: &str,
    pending: Option<&OfficeCliPendingCleanup>,
) -> bool {
    let tokens = split_shell_tokens(command);
    let mut iter = tokens.iter().map(String::as_str);
    let Some(program) = iter.next() else {
        return false;
    };
    if !matches!(program.to_ascii_lowercase().as_str(), "officecli" | "officecli.exe") {
        return false;
    }
    if iter.next() != Some("raw-set") {
        return false;
    }
    let Some(file) = iter.next() else { return false };
    if let Some(pending) = pending {
        if pending.file != file {
            return false;
        }
    }
    let Some(part) = iter.next() else { return false };
    if part != "docProps/app.xml" {
        return false;
    }
    let rest: Vec<&str> = iter.collect();
    let has_xpath = rest
        .windows(2)
        .any(|window| window[0] == "--xpath" && window[1].contains("Application"));
    let has_delete = rest
        .windows(2)
        .any(|window| window[0] == "--action" && window[1] == "delete");
    has_xpath && has_delete
}

fn office_cli_pending_reminder(
    pending: &mut Option<OfficeCliPendingCleanup>,
    tool: BuiltinTool,
) -> Option<String> {
    let entry = pending.as_ref()?;
    if entry.created_at.elapsed().as_secs() > OFFICECLI_WPS_TTL_SECONDS {
        *pending = None;
        return None;
    }
    // Do not emit a reminder on the officecli success call that just created the file.
    if tool == BuiltinTool::OfficeCli {
        // caller logic already handled create detection; if we land here it means the call
        // was an unrelated officecli command that did not clear the pending flag.
    }
    Some(format!(
        "officecli created `{}` but has not run the WPS compatibility cleanup yet. Run `officecli raw-set {} docProps/app.xml --xpath \"//ap:Application\" --action delete` before moving on; otherwise WPS Office will refuse to open the file.",
        entry.file, entry.file
    ))
}

fn cdp_command_resets_stuck_counter(command: &str) -> bool {
    let tokens = split_shell_tokens(command);
    let Some(first) = tokens.first() else { return false };
    let lower = first.to_ascii_lowercase();
    matches!(lower.as_str(), "stop" | "open" | "launch")
}

fn cdp_output_indicates_stuck(stdout: &str, stderr: &str) -> bool {
    let haystack = format!("{} {}", stdout, stderr).to_ascii_lowercase();
    haystack.contains("timed out")
        || haystack.contains("timeout")
        || haystack.contains("target closed")
        || haystack.contains("target crashed")
        || haystack.contains("no target")
        || haystack.contains("target not found")
        || haystack.contains("websocket")
}

