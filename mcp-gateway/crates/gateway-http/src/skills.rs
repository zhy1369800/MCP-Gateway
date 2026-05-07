use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use gateway_core::{
    wrap_windows_powershell_command_for_utf8, AppError, ErrorCode, GatewayConfig, SkillCommandRule,
    SkillPolicyAction, SkillsConfig,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{Notify, RwLock};
use utoipa::ToSchema;
use uuid::Uuid;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(target_os = "windows")]
fn configure_skill_command(command: &mut Command) {
    // Keep skill scripts headless on Windows to avoid flashing cmd/powershell windows.
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn configure_skill_command(_command: &mut Command) {}

#[derive(Clone, Default)]
pub struct SkillsService {
    confirmations: Arc<RwLock<HashMap<String, ConfirmationEntry>>>,
    discovery_cache: Arc<RwLock<Option<SkillDiscoveryCache>>>,
}

#[derive(Debug, Clone)]
struct ConfirmationEntry {
    /// 命令指纹：skill|command_preview，用于去重
    fingerprint: String,
    record: SkillConfirmation,
    notify: Arc<Notify>,
    timed_out: bool,
}

#[derive(Debug, Clone)]
struct SkillDiscoveryCache {
    signature: String,
    discovered: Vec<DiscoveredSkill>,
    expires_at: Instant,
}

#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillConfirmation {
    pub id: String,
    pub status: ConfirmationStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub kind: String,
    pub skill: String,
    pub display_name: String,
    pub args: Vec<String>,
    pub raw_command: String,
    pub cwd: String,
    pub affected_paths: Vec<String>,
    pub preview: String,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub skill: String,
    pub description: String,
    pub root: String,
    pub path: String,
    pub has_scripts: bool,
}

#[derive(Debug, Clone)]
struct DiscoveredSkill {
    skill: String,
    frontmatter_name: String,
    description: String,
    frontmatter_metadata: String,
    frontmatter_block: String,
    root: PathBuf,
    path: PathBuf,
    has_scripts: bool,
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillCommandArgs {
    cmd: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinShellArgs {
    cmd: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyPatchArgs {
    patch: String,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug)]
struct ToolResult {
    text: String,
    structured: Value,
    is_error: bool,
}

#[derive(Debug)]
enum PolicyDecision {
    Allow,
    Confirm(String),
    Deny(String),
}

#[derive(Debug, Clone)]
struct CommandInvocation {
    tokens: Vec<String>,
    raw: String,
    source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltinTool {
    ShellCommand,
    ApplyPatch,
}

#[derive(Debug, Clone)]
struct ConfirmationMetadata {
    kind: String,
    cwd: String,
    affected_paths: Vec<String>,
    preview: String,
}

#[derive(Debug)]
enum PatchHunk {
    AddFile {
        path: String,
        contents: Vec<String>,
    },
    DeleteFile {
        path: String,
    },
    UpdateFile {
        path: String,
        move_path: Option<String>,
        chunks: Vec<PatchChunk>,
    },
}

#[derive(Debug, Default)]
struct PatchChunk {
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

#[derive(Debug)]
struct ParsedPatch {
    hunks: Vec<PatchHunk>,
}

#[derive(Debug)]
struct PatchSummary {
    added: Vec<String>,
    modified: Vec<String>,
    deleted: Vec<String>,
}

#[derive(Debug)]
enum ConfirmationWaitOutcome {
    Approved,
    Rejected,
    TimedOut,
}

/// `create_confirmation` 的三种结果：
/// - `Created(record)`  — 新建了一条 Pending 确认，需要等用户决定
/// - `Reused(record)`   — 同指纹已有 Pending 条目，复用它，继续等待
/// - `AlreadyTimedOut(id)` — 同指纹的上一次请求刚超时，直接拒绝，不再弹窗
#[derive(Debug)]
enum CreateConfirmationResult {
    Created(SkillConfirmation),
    Reused(SkillConfirmation),
    AlreadyTimedOut(String),
}

#[derive(Debug, Clone, Default)]
struct ParsedFrontmatter {
    name: String,
    description: String,
    metadata: String,
    block: String,
}

#[derive(Debug)]
struct StreamCapturedOutput {
    text: String,
    truncated: bool,
}

#[derive(Debug, Default)]
struct StreamCaptureState {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(Debug)]
struct SkillCommandExecution {
    status: std::process::ExitStatus,
    stdout: StreamCapturedOutput,
    stderr: StreamCapturedOutput,
}

impl SkillsService {
    const CONFIRMATION_DECISION_TIMEOUT: Duration = Duration::from_secs(60);
    const CONFIRMATION_STALE_PENDING_WINDOW: Duration = Duration::from_secs(75);
    const CONFIRMATION_RESOLVED_RETENTION_WINDOW: Duration = Duration::from_secs(120);
    const SKILL_DISCOVERY_CACHE_TTL: Duration = Duration::from_secs(3);

    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_skills_server(&self, config: &GatewayConfig, server_name: &str) -> bool {
        config.skills.enabled && config.skills.server_name == server_name
    }

    pub async fn handle_mcp_request(&self, config: &GatewayConfig, request: Value) -> Value {
        let Some(object) = request.as_object() else {
            return jsonrpc_error(Value::Null, -32600, "invalid request payload", None);
        };

        let id = object.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return jsonrpc_error(id, -32600, "missing jsonrpc method", None);
        };

        if !config.skills.enabled {
            return jsonrpc_error(id, -32001, "skills server is disabled", None);
        }

        match method {
            "initialize" => jsonrpc_result(
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": "mcp-gateway-skills",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            ),
            "ping" | "notifications/initialized" => jsonrpc_result(id, json!({"ok": true})),
            "tools/list" => {
                let discovered = match self.discover_skills(&config.skills).await {
                    Ok(skills) => skills,
                    Err(error) => {
                        return jsonrpc_error(
                            id,
                            -32603,
                            "failed to discover skills",
                            Some(json!({"detail": error.to_string()})),
                        );
                    }
                };
                let summaries = summarize_discovered_skills(&discovered);
                jsonrpc_result(
                    id,
                    json!({
                        "tools": tool_definitions(&discovered),
                        "skills": summaries
                    }),
                )
            }
            "tools/call" => {
                let params = object.get("params").cloned().unwrap_or(Value::Null);
                let tool_params: ToolCallParams = match serde_json::from_value(params) {
                    Ok(value) => value,
                    Err(error) => {
                        return jsonrpc_error(
                            id,
                            -32602,
                            "invalid tool call params",
                            Some(json!({"detail": error.to_string()})),
                        );
                    }
                };

                let result = match self.execute_tool_call(config, tool_params).await {
                    Ok(output) => output,
                    Err(error) => error_to_tool_result(error),
                };
                jsonrpc_result(
                    id,
                    json!({
                        "isError": result.is_error,
                        "content": [
                            {
                                "type": "text",
                                "text": result.text
                            }
                        ],
                        "structuredContent": result.structured
                    }),
                )
            }
            _ => jsonrpc_error(id, -32601, "method not found", None),
        }
    }

    pub async fn list_pending_confirmations(&self) -> Vec<SkillConfirmation> {
        let now = Utc::now();
        let mut guard = self.confirmations.write().await;
        Self::prune_confirmations_locked(&mut guard, now);
        let mut list = guard
            .values()
            .filter(|entry| entry.record.status == ConfirmationStatus::Pending)
            .map(|entry| entry.record.clone())
            .collect::<Vec<_>>();
        list.sort_by_key(|entry| std::cmp::Reverse(entry.created_at));
        list
    }

    pub async fn approve_confirmation(&self, id: &str) -> Result<SkillConfirmation, AppError> {
        let now = Utc::now();
        let mut guard = self.confirmations.write().await;
        Self::prune_confirmations_locked(&mut guard, now);
        let Some(entry) = guard.get_mut(id) else {
            return Err(AppError::NotFound("confirmation not found".to_string()));
        };
        let notify = entry.notify.clone();
        match entry.record.status {
            ConfirmationStatus::Pending => {}
            ConfirmationStatus::Approved => {
                return Err(AppError::Conflict(
                    "confirmation already approved".to_string(),
                ));
            }
            ConfirmationStatus::Rejected => {
                return Err(AppError::Conflict(
                    "confirmation already rejected".to_string(),
                ));
            }
        }
        entry.record.status = ConfirmationStatus::Approved;
        entry.record.updated_at = now;
        entry.timed_out = false;
        let updated = entry.record.clone();
        notify.notify_one();
        Ok(updated)
    }

    pub async fn reject_confirmation(&self, id: &str) -> Result<SkillConfirmation, AppError> {
        let now = Utc::now();
        let mut guard = self.confirmations.write().await;
        Self::prune_confirmations_locked(&mut guard, now);
        let Some(target) = guard.get(id).map(|entry| entry.record.clone()) else {
            return Err(AppError::NotFound("confirmation not found".to_string()));
        };
        match target.status {
            ConfirmationStatus::Pending => {}
            ConfirmationStatus::Approved => {
                return Err(AppError::Conflict(
                    "confirmation already approved".to_string(),
                ));
            }
            ConfirmationStatus::Rejected => {
                return Err(AppError::Conflict(
                    "confirmation already rejected".to_string(),
                ));
            }
        }
        let mut notifies = Vec::new();
        for entry in guard.values_mut() {
            if entry.record.status != ConfirmationStatus::Pending {
                continue;
            }
            if Self::is_same_confirmation_signature(&entry.record, &target) {
                entry.record.status = ConfirmationStatus::Rejected;
                entry.record.updated_at = now;
                entry.timed_out = false;
                notifies.push(entry.notify.clone());
            }
        }
        for notify in notifies {
            notify.notify_one();
        }

        guard
            .get(id)
            .map(|entry| entry.record.clone())
            .ok_or_else(|| AppError::NotFound("confirmation not found".to_string()))
    }

    pub async fn list_skills_for_admin(
        &self,
        config: &GatewayConfig,
    ) -> Result<Vec<SkillSummary>, AppError> {
        self.discover_skills(&config.skills)
            .await
            .map(|skills| summarize_discovered_skills(&skills))
    }

    async fn execute_tool_call(
        &self,
        config: &GatewayConfig,
        params: ToolCallParams,
    ) -> Result<ToolResult, AppError> {
        if let Some(tool) = BuiltinTool::from_name(&params.name) {
            return self
                .execute_builtin_tool(config, tool, params.arguments)
                .await;
        }

        let skills = self.discover_skills(&config.skills).await?;
        let bindings = build_skill_tool_bindings(&skills);
        let Some((tool_name, skill)) = bindings
            .iter()
            .find(|(tool_name, _)| tool_name.eq_ignore_ascii_case(params.name.as_str()))
            .map(|(tool_name, skill)| (tool_name.clone(), (*skill).clone()))
        else {
            return Err(AppError::BadRequest(format!(
                "unknown tool name: {}",
                params.name
            )));
        };

        let args = decode_tool_args::<SkillCommandArgs>(&params.arguments)?;
        self.handle_skill_command(config, &tool_name, &skill, args)
            .await
    }

    async fn execute_builtin_tool(
        &self,
        config: &GatewayConfig,
        tool: BuiltinTool,
        arguments: Value,
    ) -> Result<ToolResult, AppError> {
        match tool {
            BuiltinTool::ShellCommand => {
                let args = decode_tool_args::<BuiltinShellArgs>(&arguments)?;
                self.handle_builtin_shell_command(config, args).await
            }
            BuiltinTool::ApplyPatch => {
                let args = decode_tool_args::<ApplyPatchArgs>(&arguments)?;
                self.handle_builtin_apply_patch(config, args).await
            }
        }
    }

    async fn handle_builtin_shell_command(
        &self,
        config: &GatewayConfig,
        args: BuiltinShellArgs,
    ) -> Result<ToolResult, AppError> {
        let command_preview = args.cmd.trim().to_string();
        if command_preview.is_empty() {
            return Err(AppError::BadRequest("cmd cannot be empty".to_string()));
        }

        if let Some(result) = missing_cwd_result_if_ambiguous(
            BuiltinTool::ShellCommand,
            &config.skills,
            args.cwd.as_deref(),
        ) {
            return Ok(result);
        }
        let cwd = resolve_builtin_cwd(&config.skills, args.cwd.as_deref())?;

        if let Some(patch) = extract_apply_patch_from_shell_command(&command_preview) {
            return self
                .execute_apply_patch_text(config, patch, &cwd, &command_preview)
                .await;
        }

        let tokens = split_shell_tokens(&command_preview);
        if tokens.is_empty() {
            return Err(AppError::BadRequest("cmd cannot be empty".to_string()));
        }
        let program = tokens[0].clone();
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
                        "command": command_preview,
                        "cwd": normalize_display_path(&cwd),
                        "policyAction": "deny"
                    }),
                ));
            }
            PolicyDecision::Confirm(reason) => {
                let metadata = ConfirmationMetadata {
                    kind: "shell".to_string(),
                    cwd: normalize_display_path(&cwd),
                    affected_paths: Vec::new(),
                    preview: command_preview.clone(),
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
                        return Ok(confirmation_rejected_result(&confirmation_id, false));
                    }
                    ConfirmationWaitOutcome::TimedOut => {
                        return Ok(confirmation_rejected_result(&confirmation_id, true));
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
        let (runner, runner_args) = shell_command_for_current_os(&command_preview);

        let started = Instant::now();
        let mut command = Command::new(&runner);
        command
            .args(&runner_args)
            .current_dir(&cwd)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        configure_skill_command(&mut command);

        let disable_truncation = should_disable_output_truncation(&program, &command_args);
        let output = execute_skill_command(
            &mut command,
            timeout_ms,
            max_output_bytes,
            disable_truncation,
        )
        .await?;
        let duration_ms = started.elapsed().as_millis() as u64;
        let stdout = output.stdout.text;
        let stderr = output.stderr.text;
        let exit_code = output.status.code().unwrap_or(-1);

        let structured = json!({
            "status": if output.status.success() { "completed" } else { "failed" },
            "tool": BuiltinTool::ShellCommand.name(),
            "command": command_preview,
            "cwd": normalize_display_path(&cwd),
            "exitCode": exit_code,
            "durationMs": duration_ms,
            "stdoutTruncated": output.stdout.truncated,
            "stderrTruncated": output.stderr.truncated
        });
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

    async fn handle_builtin_apply_patch(
        &self,
        config: &GatewayConfig,
        args: ApplyPatchArgs,
    ) -> Result<ToolResult, AppError> {
        if let Some(result) = missing_cwd_result_if_ambiguous(
            BuiltinTool::ApplyPatch,
            &config.skills,
            args.cwd.as_deref(),
        ) {
            return Ok(result);
        }
        let cwd = resolve_builtin_cwd(&config.skills, args.cwd.as_deref())?;
        self.execute_apply_patch_text(config, args.patch, &cwd, "apply_patch")
            .await
    }

    async fn execute_apply_patch_text(
        &self,
        config: &GatewayConfig,
        patch: String,
        cwd: &Path,
        raw_command: &str,
    ) -> Result<ToolResult, AppError> {
        let patch_preview = patch.trim().to_string();
        if patch_preview.is_empty() {
            return Err(AppError::BadRequest("patch cannot be empty".to_string()));
        }

        let parsed = parse_apply_patch(&patch_preview)?;
        let affected_paths = patch_affected_paths(&parsed, cwd)?;
        let access_decision = evaluate_paths_policy(&config.skills, &affected_paths);
        match access_decision {
            PolicyDecision::Deny(reason) => {
                return Ok(tool_error(
                    mcp_gateway_policy_denied_text(&reason),
                    json!({
                        "status": "blocked",
                        "reason": reason,
                        "tool": BuiltinTool::ApplyPatch.name(),
                        "cwd": normalize_display_path(cwd),
                        "affectedPaths": affected_paths.iter().map(|path| normalize_display_path(path)).collect::<Vec<_>>()
                    }),
                ));
            }
            PolicyDecision::Confirm(reason) => {
                let metadata = ConfirmationMetadata {
                    kind: "patch".to_string(),
                    cwd: normalize_display_path(cwd),
                    affected_paths: affected_paths
                        .iter()
                        .map(|path| normalize_display_path(path))
                        .collect(),
                    preview: truncate_preview(&patch_preview, 4000),
                };
                let confirmation_id = match self
                    .create_confirmation_with_metadata(
                        "builtin:apply_patch",
                        "Apply Patch",
                        &[String::from("apply_patch")],
                        raw_command,
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
                        return Ok(confirmation_rejected_result(&confirmation_id, false));
                    }
                    ConfirmationWaitOutcome::TimedOut => {
                        return Ok(confirmation_rejected_result(&confirmation_id, true));
                    }
                }
            }
            PolicyDecision::Allow => {}
        }

        let summary = apply_parsed_patch(&parsed, cwd)?;
        let text = patch_summary_text(&summary);
        Ok(tool_success(
            text,
            json!({
                "status": "completed",
                "tool": BuiltinTool::ApplyPatch.name(),
                "cwd": normalize_display_path(cwd),
                "added": summary.added,
                "modified": summary.modified,
                "deleted": summary.deleted
            }),
        ))
    }

    async fn handle_skill_command(
        &self,
        config: &GatewayConfig,
        tool_name: &str,
        skill: &DiscoveredSkill,
        args: SkillCommandArgs,
    ) -> Result<ToolResult, AppError> {
        let command_preview = args.cmd.trim().to_string();
        if command_preview.is_empty() {
            return Err(AppError::BadRequest("cmd cannot be empty".to_string()));
        }

        let tokens = split_shell_tokens(&command_preview);
        if tokens.is_empty() {
            return Err(AppError::BadRequest("cmd cannot be empty".to_string()));
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
                        "command": command_preview,
                        "policyAction": "deny",
                        "nextStep": "把匹配的 skills.policy 规则或 skills.policy.defaultAction 改为 confirm 让用户确认运行，或改为 allow 让它默认运行，然后重试。"
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
                        return Ok(confirmation_rejected_result(&confirmation_id, false));
                    }
                    ConfirmationWaitOutcome::TimedOut => {
                        return Ok(confirmation_rejected_result(&confirmation_id, true));
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
        configure_skill_command(&mut command);

        let disable_truncation = should_disable_output_truncation(&program, &command_args);
        let output = execute_skill_command(
            &mut command,
            timeout_ms,
            max_output_bytes,
            disable_truncation,
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
        if stdout_truncated {
            structured.insert("stdoutTruncated".to_string(), Value::Bool(true));
        }
        if stderr_truncated {
            structured.insert("stderrTruncated".to_string(), Value::Bool(true));
        }
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

    #[cfg(test)]
    async fn create_confirmation(
        &self,
        skill: &str,
        display_name: &str,
        args: &[String],
        raw_command: &str,
        reason: &str,
    ) -> CreateConfirmationResult {
        self.create_confirmation_with_metadata(
            skill,
            display_name,
            args,
            raw_command,
            reason,
            ConfirmationMetadata {
                kind: "skill".to_string(),
                cwd: String::new(),
                affected_paths: Vec::new(),
                preview: raw_command.to_string(),
            },
        )
        .await
    }

    async fn create_confirmation_with_metadata(
        &self,
        skill: &str,
        display_name: &str,
        args: &[String],
        raw_command: &str,
        reason: &str,
        metadata: ConfirmationMetadata,
    ) -> CreateConfirmationResult {
        let fingerprint = format!("{skill}|{raw_command}");
        let now = Utc::now();
        let mut guard = self.confirmations.write().await;
        Self::prune_confirmations_locked(&mut guard, now);

        // 检查同指纹是否已有条目：
        // - Pending  → 复用，不重复弹窗
        // - 刚超时的 Rejected (timed_out=true) → 直接告知调用方已超时，不新建
        // - 用户手动 Rejected / Approved → 允许重新发起
        for entry in guard.values() {
            if entry.fingerprint != fingerprint {
                continue;
            }
            match entry.record.status {
                ConfirmationStatus::Pending => {
                    return CreateConfirmationResult::Reused(entry.record.clone());
                }
                ConfirmationStatus::Rejected if entry.timed_out => {
                    return CreateConfirmationResult::AlreadyTimedOut(entry.record.id.clone());
                }
                _ => {}
            }
        }

        let id = Uuid::new_v4().to_string();
        let record = SkillConfirmation {
            id: id.clone(),
            status: ConfirmationStatus::Pending,
            created_at: now,
            updated_at: now,
            kind: metadata.kind,
            skill: skill.to_string(),
            display_name: display_name.to_string(),
            args: args.to_vec(),
            raw_command: raw_command.to_string(),
            cwd: metadata.cwd,
            affected_paths: metadata.affected_paths,
            preview: metadata.preview,
            reason: reason.to_string(),
        };

        guard.insert(
            id,
            ConfirmationEntry {
                fingerprint,
                record: record.clone(),
                notify: Arc::new(Notify::new()),
                timed_out: false,
            },
        );
        let timeout_service = self.clone();
        let timeout_id = record.id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Self::CONFIRMATION_DECISION_TIMEOUT).await;
            timeout_service
                .reject_confirmation_on_timeout(&timeout_id)
                .await;
        });
        CreateConfirmationResult::Created(record)
    }

    async fn wait_for_confirmation_decision(
        &self,
        confirmation_id: &str,
        timeout: Duration,
        _poll_interval: Duration,
    ) -> ConfirmationWaitOutcome {
        let started = Instant::now();
        loop {
            let wait_notify = {
                let now = Utc::now();
                let mut guard = self.confirmations.write().await;
                Self::prune_confirmations_locked(&mut guard, now);
                match guard.get(confirmation_id).map(|entry| {
                    (
                        entry.record.status.clone(),
                        entry.record.created_at,
                        entry.notify.clone(),
                        entry.timed_out,
                    )
                }) {
                    Some((ConfirmationStatus::Approved, _, _, _)) => {
                        guard.remove(confirmation_id);
                        return ConfirmationWaitOutcome::Approved;
                    }
                    Some((ConfirmationStatus::Rejected, _, _, timed_out)) => {
                        guard.remove(confirmation_id);
                        return if timed_out {
                            ConfirmationWaitOutcome::TimedOut
                        } else {
                            ConfirmationWaitOutcome::Rejected
                        };
                    }
                    Some((ConfirmationStatus::Pending, created_at, notify, _)) => {
                        if Self::age_exceeds(created_at, now, timeout) {
                            if let Some(entry) = guard.get_mut(confirmation_id) {
                                entry.record.status = ConfirmationStatus::Rejected;
                                entry.record.updated_at = now;
                                entry.timed_out = true;
                                entry.notify.notify_one();
                            }
                            return ConfirmationWaitOutcome::TimedOut;
                        }
                        notify
                    }
                    None => return ConfirmationWaitOutcome::TimedOut,
                }
            };

            let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
                let mut guard = self.confirmations.write().await;
                if let Some(entry) = guard.get_mut(confirmation_id) {
                    entry.record.status = ConfirmationStatus::Rejected;
                    entry.record.updated_at = Utc::now();
                    entry.timed_out = true;
                    entry.notify.notify_one();
                }
                return ConfirmationWaitOutcome::TimedOut;
            };
            if remaining.is_zero() {
                let mut guard = self.confirmations.write().await;
                if let Some(entry) = guard.get_mut(confirmation_id) {
                    entry.record.status = ConfirmationStatus::Rejected;
                    entry.record.updated_at = Utc::now();
                    entry.timed_out = true;
                    entry.notify.notify_one();
                }
                return ConfirmationWaitOutcome::TimedOut;
            }

            let notified = tokio::time::timeout(remaining, wait_notify.notified()).await;
            if notified.is_err() {
                let mut guard = self.confirmations.write().await;
                if let Some(entry) = guard.get_mut(confirmation_id) {
                    entry.record.status = ConfirmationStatus::Rejected;
                    entry.record.updated_at = Utc::now();
                    entry.timed_out = true;
                    entry.notify.notify_one();
                }
                return ConfirmationWaitOutcome::TimedOut;
            }
        }
    }

    async fn reject_confirmation_on_timeout(&self, id: &str) {
        let now = Utc::now();
        let mut guard = self.confirmations.write().await;
        Self::prune_confirmations_locked(&mut guard, now);
        let Some(entry) = guard.get_mut(id) else {
            return;
        };
        if entry.record.status != ConfirmationStatus::Pending {
            return;
        }
        entry.record.status = ConfirmationStatus::Rejected;
        entry.record.updated_at = now;
        entry.timed_out = true;
        entry.notify.notify_one();
    }

    fn age_exceeds(created_at: DateTime<Utc>, now: DateTime<Utc>, ttl: Duration) -> bool {
        now.signed_duration_since(created_at)
            .to_std()
            .map(|elapsed| elapsed >= ttl)
            .unwrap_or(false)
    }

    fn is_same_confirmation_signature(left: &SkillConfirmation, right: &SkillConfirmation) -> bool {
        left.skill == right.skill
            && left.display_name == right.display_name
            && left.args == right.args
            && left.raw_command == right.raw_command
            && left.reason == right.reason
    }

    fn prune_confirmations_locked(
        confirmations: &mut HashMap<String, ConfirmationEntry>,
        now: DateTime<Utc>,
    ) {
        confirmations.retain(|_, entry| match entry.record.status {
            ConfirmationStatus::Pending => !Self::age_exceeds(
                entry.record.created_at,
                now,
                Self::CONFIRMATION_STALE_PENDING_WINDOW,
            ),
            ConfirmationStatus::Approved | ConfirmationStatus::Rejected => !Self::age_exceeds(
                entry.record.updated_at,
                now,
                Self::CONFIRMATION_RESOLVED_RETENTION_WINDOW,
            ),
        });
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

fn tool_error(text: String, structured: Value) -> ToolResult {
    ToolResult {
        text,
        structured,
        is_error: true,
    }
}

fn confirmation_rejected_result(confirmation_id: &str, timed_out: bool) -> ToolResult {
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
    tool_success(
        text.to_string(),
        json!({
            "status": "rejected",
            "reason": reason,
            "confirmationId": confirmation_id
        }),
    )
}

fn mcp_gateway_policy_denied_text(reason: &str) -> String {
    format!(
        "MCP Gateway 已拒绝此命令：命令命中了网关拒绝策略或默认拒绝规则，因此没有执行。原因：{reason}。要执行该命令，请把匹配的 skills.policy 规则或 skills.policy.defaultAction 改为 \"confirm\" 让用户确认运行，或改为 \"allow\" 让它默认运行，然后重试。"
    )
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

fn tool_definitions(skills: &[DiscoveredSkill]) -> Value {
    let bindings = build_skill_tool_bindings(skills);
    let now = Utc::now().to_rfc3339();
    let os = current_os_label();
    let mut tools = builtin_tool_definitions(os, &now);

    tools.extend(bindings.into_iter().map(|(tool_name, skill)| {
        let description = render_skill_tool_description(skill, os, &now);
        json!({
            "name": tool_name,
            "description": description,
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["cmd"],
                "properties": {
                    "cmd": {
                        "type": "string",
                        "description": "Shell command string for this skill. Main uses: read markdown files with full paths (for example `cat D:/.../SKILL.md` or `Get-Content D:/.../SKILL.md`) and run scripts."
                    }
                }
            }
        })
    }));

    Value::Array(tools)
}

fn builtin_tool_definitions(os: &str, now: &str) -> Vec<Value> {
    let shell_description = format!(
        r#"Bundled Skill: terminal operations.
This bundled skill has no separate SKILL.md file; follow this description as the full usage guide.

Scope and cwd:
- The user-configured allowed directories are the only folders this skill can operate in.
- Always set cwd to the concrete directory you intend to operate in.
- If multiple allowed directories exist and the user has not specified which folder to operate in, ask the user to choose one before calling this tool.
- If the requested file or folder is outside every allowed directory, ask the user to add the corresponding directory to allowed directories before continuing.

Command usage:
- Use non-interactive commands only; avoid commands that wait for prompts or open full-screen editors.
- Quote paths that contain spaces or non-ASCII characters.
- Prefer fast discovery commands: rg --files for listing project files, rg for text search. If rg is unavailable, use the platform shell's normal alternatives.
- For reading files on Windows, prefer Get-Content -Raw <path>; on Unix-like systems, use cat, sed -n, or similar read-only commands.
- Prefer the apply_patch bundled skill for file edits instead of shell redirection or in-place text tools when a structured edit is possible.
- Destructive or sensitive commands may be blocked or require user approval by policy.

Current OS: {os}. Current datetime: {now}."#
    );
    let patch_description = format!(
        r#"Bundled Skill: structured file editing.
This bundled skill has no separate SKILL.md file; follow this description as the full usage guide.

Scope and cwd:
- All affected files must be inside the user-configured allowed directories.
- Always set cwd to the concrete directory for relative patch paths.
- If multiple allowed directories exist and the user has not specified which folder to edit, ask the user to choose one before calling this tool.
- If the requested file is outside every allowed directory, ask the user to add the corresponding directory to allowed directories before continuing.

Patch format:
- This tool does not accept standard unified diff headers such as --- file and +++ file.
- Use only the format below. Every patch starts with *** Begin Patch and ends with *** End Patch.
- Add files with *** Add File: path, where every content line starts with +.
- Delete files with *** Delete File: path.
- Update files with *** Update File: path. Inside update hunks, unchanged context lines start with one space, removed lines start with -, and added lines start with +.
- Move files by adding *** Move to: new-path immediately after *** Update File: old-path.

Minimal replacement example:
*** Begin Patch
*** Update File: index.html
@@
-<h1>Old title</h1>
+<h1>New title</h1>
*** End Patch

Add file example:
*** Begin Patch
*** Add File: notes.txt
+first line
+second line
*** End Patch

Current OS: {os}. Current datetime: {now}."#
    );
    vec![
        json!({
            "name": BuiltinTool::ShellCommand.name(),
            "description": shell_description,
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["cmd"],
                "properties": {
                    "cmd": {
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
                    }
                }
            }
        }),
        json!({
            "name": BuiltinTool::ApplyPatch.name(),
            "description": patch_description,
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["patch"],
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "Structured patch text. Must use *** Add File, *** Delete File, or *** Update File blocks. Do not send standard unified diff headers like --- file and +++ file."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Concrete working directory for relative patch paths. It must be inside one configured allowed directory. Required when more than one allowed directory exists; do not omit it in that case."
                    }
                }
            }
        }),
    ]
}

fn build_skill_tool_bindings(skills: &[DiscoveredSkill]) -> Vec<(String, &DiscoveredSkill)> {
    let mut sorted = skills.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|skill| skill.skill.to_ascii_lowercase());

    let mut used = HashMap::<String, usize>::new();
    let mut bindings = Vec::with_capacity(sorted.len());
    for skill in sorted {
        let base = skill_tool_name_base(skill);
        let next = used
            .entry(base.clone())
            .and_modify(|count| *count += 1)
            .or_insert(1);
        let tool_name = if *next == 1 {
            base
        } else {
            format!("{}_{}", base, *next)
        };
        bindings.push((tool_name, skill));
    }
    bindings
}

fn skill_tool_name_base(skill: &DiscoveredSkill) -> String {
    sanitize_tool_name(skill_display_name(skill))
}

impl BuiltinTool {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            value if value.eq_ignore_ascii_case(Self::ShellCommand.name()) => {
                Some(Self::ShellCommand)
            }
            value if value.eq_ignore_ascii_case(Self::ApplyPatch.name()) => Some(Self::ApplyPatch),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::ShellCommand => "shell_command",
            Self::ApplyPatch => "apply_patch",
        }
    }
}

fn skill_display_name(skill: &DiscoveredSkill) -> &str {
    let frontmatter_name = skill.frontmatter_name.trim();
    if frontmatter_name.is_empty() {
        skill.skill.trim()
    } else {
        frontmatter_name
    }
}

fn sanitize_tool_name(raw: &str) -> String {
    let mut out = String::new();
    let mut last_separator = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_separator = false;
            continue;
        }
        if matches!(ch, '-' | '_') {
            out.push(ch);
            last_separator = false;
            continue;
        }
        if !last_separator {
            out.push('_');
            last_separator = true;
        }
    }

    let trimmed = out.trim_matches('_').trim_matches('-').to_string();
    if trimmed.is_empty() {
        "skill".to_string()
    } else {
        trimmed
    }
}

fn render_skill_tool_description(skill: &DiscoveredSkill, os: &str, now: &str) -> String {
    let meta_description = if skill.description.trim().is_empty() {
        format!("Skill instructions for {}", skill.skill)
    } else {
        skill.description.trim().to_string()
    };
    let frontmatter_block = if skill.frontmatter_block.trim().is_empty() {
        "none".to_string()
    } else {
        format!("---\n{}\n---", skill.frontmatter_block.trim())
    };
    let skill_path = normalize_display_path(&skill.path);
    format!(
        "To learn the complete usage of this skill, run `cmd` to read the full SKILL.md text. like `cat /.../SKILL.md` or `Get-Content D:/.../SKILL.md`. The `cmd` value should be one shell command string used either to read markdown files or run scripts.\nCurrent OS: {os}.\nCurrent datetime: {now}.\nSkill path: {skill_path}.\nFront matter summary:\nname: {}\ndescription: {}\nmetadata: {}\nFront matter raw (YAML):\n{}",
        skill_display_name(skill),
        meta_description,
        if skill.frontmatter_metadata.trim().is_empty() {
            "none"
        } else {
            skill.frontmatter_metadata.trim()
        },
        frontmatter_block
    )
}

fn current_os_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Unknown"
    }
}

fn normalize_display_path(path: &Path) -> String {
    let raw = path.to_string_lossy().to_string();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    raw
}

fn jsonrpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn jsonrpc_error(id: Value, code: i32, message: &str, data: Option<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
            "data": data
        }
    })
}

fn discover_skills_sync(roots: &[String]) -> Result<Vec<DiscoveredSkill>, AppError> {
    let mut discovered = Vec::new();
    let mut seen_skill_dirs = HashSet::new();

    for root in roots {
        let root_path = PathBuf::from(root);
        if !root_path.is_dir() {
            continue;
        }

        let mut stack = vec![root_path.clone()];
        let mut seen_dirs = HashSet::new();

        while let Some(current_dir) = stack.pop() {
            let canonical_dir =
                std::fs::canonicalize(&current_dir).unwrap_or_else(|_| current_dir.clone());
            if !seen_dirs.insert(canonical_dir.clone()) {
                continue;
            }

            register_skill_directory(
                &root_path,
                &canonical_dir,
                &mut seen_skill_dirs,
                &mut discovered,
            );

            let entries = match std::fs::read_dir(&canonical_dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            for entry in entries {
                let Ok(entry) = entry else {
                    continue;
                };
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() || file_type.is_symlink() {
                    continue;
                }
                stack.push(entry.path());
            }
        }
    }

    discovered.sort_by_key(|entry| entry.skill.to_lowercase());
    Ok(discovered)
}

fn register_skill_directory(
    root_path: &Path,
    dir_path: &Path,
    seen_skill_dirs: &mut HashSet<PathBuf>,
    discovered: &mut Vec<DiscoveredSkill>,
) {
    let skill_md = dir_path.join("SKILL.md");
    if !skill_md.is_file() {
        return;
    }

    let canonical_skill_dir = std::fs::canonicalize(dir_path).unwrap_or_else(|_| dir_path.into());
    if !seen_skill_dirs.insert(canonical_skill_dir.clone()) {
        return;
    }

    let dir_name = canonical_skill_dir
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_string)
        .unwrap_or_else(|| canonical_skill_dir.to_string_lossy().to_string());
    let parsed_frontmatter = parse_frontmatter_fields(&skill_md).unwrap_or_default();

    discovered.push(DiscoveredSkill {
        skill: dir_name.clone(),
        frontmatter_name: parsed_frontmatter.name,
        description: parsed_frontmatter.description,
        frontmatter_metadata: parsed_frontmatter.metadata,
        frontmatter_block: parsed_frontmatter.block,
        root: root_path.to_path_buf(),
        has_scripts: canonical_skill_dir.join("scripts").is_dir(),
        path: canonical_skill_dir,
    });
}

fn parse_frontmatter_fields(skill_md_path: &Path) -> Result<ParsedFrontmatter, AppError> {
    let content = std::fs::read_to_string(skill_md_path)?;
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Ok(ParsedFrontmatter::default());
    }

    let mut frontmatter_lines = Vec::new();
    let mut has_closing = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "..." {
            has_closing = true;
            break;
        }
        frontmatter_lines.push(line.to_string());
    }
    if !has_closing {
        return Ok(ParsedFrontmatter::default());
    }

    let raw = frontmatter_lines.join("\n").trim().to_string();
    if raw.trim().is_empty() {
        return Ok(ParsedFrontmatter::default());
    }

    let frontmatter: Value = serde_yaml::from_str(&raw).map_err(|error| {
        AppError::BadRequest(format!(
            "invalid YAML frontmatter in {}: {error}",
            skill_md_path.display()
        ))
    })?;
    let frontmatter_obj = frontmatter.as_object().ok_or_else(|| {
        AppError::BadRequest(format!(
            "frontmatter in {} must be a YAML mapping",
            skill_md_path.display()
        ))
    })?;

    let name = frontmatter_obj
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let description = frontmatter_obj
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let metadata = frontmatter_obj
        .get("metadata")
        .map(frontmatter_value_summary)
        .unwrap_or_else(|| "none".to_string());
    Ok(ParsedFrontmatter {
        name,
        description,
        metadata,
        block: raw,
    })
}

fn frontmatter_value_summary(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::String(text) => text.to_string(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "unserializable".to_string()),
    }
}

fn shell_command_for_current_os(cmd: &str) -> (String, Vec<String>) {
    if cfg!(target_os = "windows") {
        let runner = "powershell".to_string();
        let args = vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-Command".to_string(),
            cmd.to_string(),
        ];
        wrap_windows_powershell_command_for_utf8(&runner, &args).unwrap_or((runner, args))
    } else {
        ("sh".to_string(), vec!["-lc".to_string(), cmd.to_string()])
    }
}

fn roots_signature(roots: &[String]) -> String {
    let mut normalized = roots
        .iter()
        .map(|root| root.trim())
        .filter(|root| !root.is_empty())
        .map(|root| {
            normalize_lexical_path(Path::new(root))
                .to_string_lossy()
                .to_string()
        })
        .collect::<Vec<_>>();
    normalized.sort_by_key(|item| item.to_ascii_lowercase());
    normalized.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    normalized.join("\u{001f}")
}

async fn execute_skill_command(
    command: &mut Command,
    timeout_ms: u64,
    max_output_bytes: usize,
    disable_truncation: bool,
) -> Result<SkillCommandExecution, AppError> {
    let mut child = command
        .spawn()
        .map_err(|error| AppError::Upstream(format!("failed to execute command: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Internal("missing stdout from skill command".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Internal("missing stderr from skill command".to_string()))?;

    let stdout_state = Arc::new(Mutex::new(StreamCaptureState::default()));
    let stderr_state = Arc::new(Mutex::new(StreamCaptureState::default()));

    let stdout_task = tokio::spawn(capture_stream_output(
        stdout,
        stdout_state.clone(),
        max_output_bytes,
        disable_truncation,
    ));
    let stderr_task = tokio::spawn(capture_stream_output(
        stderr,
        stderr_state.clone(),
        max_output_bytes,
        disable_truncation,
    ));

    let status = match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await {
        Ok(wait_result) => wait_result
            .map_err(|error| AppError::Upstream(format!("failed to execute command: {error}")))?,
        Err(_) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            stdout_task.abort();
            stderr_task.abort();
            let stdout = snapshot_stream_output(&stdout_state);
            let stderr = snapshot_stream_output(&stderr_state);
            return Err(AppError::Upstream(command_timeout_text(
                timeout_ms,
                &stdout.text,
                &stderr.text,
            )));
        }
    };

    stdout_task
        .await
        .map_err(|error| AppError::Internal(format!("stdout capture join error: {error}")))??;
    stderr_task
        .await
        .map_err(|error| AppError::Internal(format!("stderr capture join error: {error}")))??;
    let stdout = snapshot_stream_output(&stdout_state);
    let stderr = snapshot_stream_output(&stderr_state);

    Ok(SkillCommandExecution {
        status,
        stdout,
        stderr,
    })
}

async fn capture_stream_output<R>(
    mut reader: R,
    shared_state: Arc<Mutex<StreamCaptureState>>,
    max_output_bytes: usize,
    disable_truncation: bool,
) -> Result<(), AppError>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut chunk = [0_u8; 8192];

    loop {
        let read = reader.read(&mut chunk).await.map_err(|error| {
            AppError::Upstream(format!("failed to read command output: {error}"))
        })?;
        if read == 0 {
            break;
        }

        let mut state = match shared_state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if disable_truncation {
            state.bytes.extend_from_slice(&chunk[..read]);
            continue;
        }

        if state.bytes.len() < max_output_bytes {
            let available = max_output_bytes - state.bytes.len();
            let take = available.min(read);
            state.bytes.extend_from_slice(&chunk[..take]);
            if take < read {
                state.truncated = true;
            }
        } else {
            state.truncated = true;
        }
    }

    Ok(())
}

fn snapshot_stream_output(shared_state: &Arc<Mutex<StreamCaptureState>>) -> StreamCapturedOutput {
    let state = match shared_state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    StreamCapturedOutput {
        text: String::from_utf8_lossy(&state.bytes).to_string(),
        truncated: state.truncated,
    }
}

fn evaluate_policy(
    skills: &SkillsConfig,
    program: &str,
    command_args: &[String],
    raw_command: &str,
    script_path: &Path,
    script_text: Option<&str>,
) -> PolicyDecision {
    let invocations = collect_command_invocations(program, command_args, raw_command, script_text);
    for invocation in &invocations {
        for rule in &skills.policy.rules {
            if rule_matches(rule, invocation) {
                let base_reason = if rule.reason.trim().is_empty() {
                    "matched command rule".to_string()
                } else {
                    rule.reason.trim().to_string()
                };
                let reason = format!(
                    "{} (rule: {}, source: {})",
                    base_reason, rule.id, invocation.source
                );
                return action_to_decision(&rule.action, reason);
            }
        }
    }

    if skills.policy.path_guard.enabled && !skills.policy.path_guard.whitelist_dirs.is_empty() {
        let whitelist = skills
            .policy
            .path_guard
            .whitelist_dirs
            .iter()
            .map(PathBuf::from)
            .map(normalize_root_path)
            .collect::<Vec<_>>();

        if let Some((token, source, resolved)) =
            find_outside_whitelist_path(script_path, command_args, script_text, &whitelist)
        {
            let reason = format!(
                "path '{}' resolved to '{}' is outside whitelist (source: {})",
                token,
                resolved.to_string_lossy(),
                source
            );
            return action_to_decision(&skills.policy.path_guard.on_violation, reason);
        }
    }

    action_to_decision(
        &skills.policy.default_action,
        "matched default policy".to_string(),
    )
}

fn action_to_decision(action: &SkillPolicyAction, reason: String) -> PolicyDecision {
    match action {
        SkillPolicyAction::Allow => PolicyDecision::Allow,
        SkillPolicyAction::Confirm => PolicyDecision::Confirm(reason),
        SkillPolicyAction::Deny => PolicyDecision::Deny(reason),
    }
}

fn collect_command_invocations(
    program: &str,
    command_args: &[String],
    raw_command: &str,
    script_text: Option<&str>,
) -> Vec<CommandInvocation> {
    let mut list = Vec::new();
    let mut runtime_tokens = Vec::with_capacity(command_args.len() + 1);
    runtime_tokens.push(normalize_command_token(program));
    runtime_tokens.extend(command_args.iter().map(|item| item.to_ascii_lowercase()));
    list.push(CommandInvocation {
        raw: std::iter::once(program.to_string())
            .chain(command_args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" "),
        tokens: runtime_tokens,
        source: "runtime".to_string(),
    });

    for (index, segment) in split_command_segments(raw_command).into_iter().enumerate() {
        let mut tokens = split_shell_tokens(&segment);
        if tokens.is_empty() {
            continue;
        }
        if tokens[0] == "&" {
            tokens.remove(0);
        }
        if tokens.is_empty() {
            continue;
        }

        let mut normalized = Vec::with_capacity(tokens.len());
        normalized.push(normalize_command_token(&tokens[0]));
        normalized.extend(tokens.iter().skip(1).map(|item| item.to_ascii_lowercase()));

        list.push(CommandInvocation {
            raw: segment,
            tokens: normalized,
            source: format!("runtime:segment:{}", index + 1),
        });
    }

    if let Some(script) = script_text {
        for (line_no, line) in script.lines().enumerate().take(300) {
            let trimmed = line.trim();
            if trimmed.is_empty() || is_comment_line(trimmed) {
                continue;
            }

            let mut tokens = split_shell_tokens(trimmed);
            if tokens.is_empty() {
                continue;
            }
            if tokens[0] == "&" {
                tokens.remove(0);
            }
            if tokens.is_empty() {
                continue;
            }

            let mut normalized = Vec::with_capacity(tokens.len());
            normalized.push(normalize_command_token(&tokens[0]));
            normalized.extend(tokens.iter().skip(1).map(|item| item.to_ascii_lowercase()));

            list.push(CommandInvocation {
                raw: trimmed.to_string(),
                tokens: normalized,
                source: format!("script:{}", line_no + 1),
            });
        }
    }

    list
}

fn split_command_segments(raw: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            current.push(ch);
            continue;
        }

        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                current.push(ch);
            }
            ';' => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    segments.push(trimmed.to_string());
                }
                current.clear();
            }
            '|' => {
                if chars.peek().copied() == Some('|') {
                    let _ = chars.next();
                }
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    segments.push(trimmed.to_string());
                }
                current.clear();
            }
            '&' => {
                if chars.peek().copied() == Some('&') {
                    let _ = chars.next();
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        segments.push(trimmed.to_string());
                    }
                    current.clear();
                } else {
                    current.push(ch);
                }
            }
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        segments.push(trimmed.to_string());
    }

    segments
}

fn rule_matches(rule: &SkillCommandRule, invocation: &CommandInvocation) -> bool {
    if !rule.command_tree.is_empty() {
        if invocation.tokens.len() < rule.command_tree.len() {
            return false;
        }
        for (idx, node) in rule.command_tree.iter().enumerate() {
            if invocation.tokens[idx] != *node {
                return false;
            }
        }
    }

    if !rule.contains.is_empty() {
        let raw = invocation.raw.to_ascii_lowercase();
        for needle in &rule.contains {
            let matched_in_tokens = invocation.tokens.iter().any(|token| token.contains(needle));
            if !matched_in_tokens && !raw.contains(needle) {
                return false;
            }
        }
    }

    true
}

fn find_outside_whitelist_path(
    script_path: &Path,
    command_args: &[String],
    script_text: Option<&str>,
    whitelist: &[PathBuf],
) -> Option<(String, String, PathBuf)> {
    let script_file = normalize_root_path(script_path.to_path_buf());
    let script_dir = script_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    for (token, source) in collect_path_candidates(command_args, script_text) {
        let resolved = resolve_candidate_path(&script_dir, &token);
        if resolved == script_file {
            continue;
        }
        let allowed = whitelist.iter().any(|root| resolved.starts_with(root));
        if !allowed {
            return Some((token, source, resolved));
        }
    }
    None
}

fn collect_path_candidates(
    command_args: &[String],
    script_text: Option<&str>,
) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    for (index, arg) in command_args.iter().enumerate() {
        for token in split_shell_tokens(arg) {
            let cleaned = strip_matching_quotes(&token);
            if is_path_like(cleaned) && seen.insert(cleaned.to_string()) {
                candidates.push((cleaned.to_string(), format!("arg:{index}")));
            }
        }
    }

    if let Some(script) = script_text {
        for (line_no, line) in script.lines().enumerate().take(300) {
            let trimmed = line.trim();
            if trimmed.is_empty() || is_comment_line(trimmed) {
                continue;
            }
            let tokens = split_shell_tokens(trimmed);
            for token in tokens.into_iter().skip(1) {
                let cleaned = strip_matching_quotes(&token);
                if is_path_like(cleaned) && seen.insert(cleaned.to_string()) {
                    candidates.push((cleaned.to_string(), format!("script:{}", line_no + 1)));
                }
            }
        }
    }

    candidates
}

fn resolve_candidate_path(script_dir: &Path, token: &str) -> PathBuf {
    let expanded = expand_home_path(token);
    let raw = PathBuf::from(expanded);
    let absolute = if raw.is_absolute() {
        raw
    } else {
        script_dir.join(raw)
    };
    normalize_root_path(absolute)
}

fn resolve_builtin_cwd(skills: &SkillsConfig, cwd: Option<&str>) -> Result<PathBuf, AppError> {
    let allowed_dirs = skills
        .policy
        .path_guard
        .whitelist_dirs
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    let selected = if let Some(cwd) = cwd.map(str::trim).filter(|value| !value.is_empty()) {
        PathBuf::from(cwd)
    } else {
        match allowed_dirs.as_slice() {
            [] => {
                return Err(AppError::Validation(
                    "skills enabled requires at least one allowed directory".to_string(),
                ));
            }
            [only] => PathBuf::from(only),
            _ => {
                return Err(AppError::BadRequest(
                    "cwd is required because multiple allowed directories are configured; ask the user which directory to operate in".to_string(),
                ));
            }
        }
    };
    let normalized = normalize_root_path(selected);
    if !normalized.exists() || !normalized.is_dir() {
        return Err(AppError::Validation(format!(
            "working directory must be an existing directory: {}",
            normalized.to_string_lossy()
        )));
    }

    match evaluate_paths_policy(skills, std::slice::from_ref(&normalized)) {
        PolicyDecision::Allow => Ok(normalized),
        PolicyDecision::Confirm(reason) | PolicyDecision::Deny(reason) => {
            Err(AppError::Validation(reason))
        }
    }
}

fn missing_cwd_result_if_ambiguous(
    tool: BuiltinTool,
    skills: &SkillsConfig,
    cwd: Option<&str>,
) -> Option<ToolResult> {
    if cwd.map(str::trim).is_some_and(|value| !value.is_empty()) {
        return None;
    }

    let allowed_dirs = configured_allowed_dirs(skills);
    if allowed_dirs.len() <= 1 {
        return None;
    }

    let message = "cwd is required because multiple allowed directories are configured; ask the user which directory to operate in";
    Some(tool_error(
        format!(
            "{message}\nAllowed directories:\n{}",
            allowed_dirs
                .iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        json!({
            "status": "error",
            "code": "BadRequest",
            "message": message,
            "tool": tool.name(),
            "allowedDirectories": allowed_dirs,
            "nextStep": "Ask the user which allowed directory should be used as cwd, then retry with cwd set to that directory or a child directory."
        }),
    ))
}

fn configured_allowed_dirs(skills: &SkillsConfig) -> Vec<String> {
    skills
        .policy
        .path_guard
        .whitelist_dirs
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| normalize_display_path(&normalize_root_path(PathBuf::from(value))))
        .collect()
}

fn evaluate_paths_policy(skills: &SkillsConfig, paths: &[PathBuf]) -> PolicyDecision {
    if !skills.policy.path_guard.enabled {
        return PolicyDecision::Allow;
    }

    let whitelist = skills
        .policy
        .path_guard
        .whitelist_dirs
        .iter()
        .map(PathBuf::from)
        .map(normalize_root_path)
        .collect::<Vec<_>>();
    if whitelist.is_empty() {
        return PolicyDecision::Deny("skills allowed directories are empty".to_string());
    }

    for path in paths {
        let resolved = normalize_root_path(path.clone());
        let allowed = whitelist.iter().any(|root| resolved.starts_with(root));
        if !allowed {
            let reason = format!(
                "path '{}' is outside allowed directories",
                resolved.to_string_lossy()
            );
            return action_to_decision(&skills.policy.path_guard.on_violation, reason);
        }
    }

    PolicyDecision::Allow
}

fn extract_apply_patch_from_shell_command(command: &str) -> Option<String> {
    let begin = command.find("*** Begin Patch")?;
    let end_marker = "*** End Patch";
    let end = command[begin..].find(end_marker)? + begin + end_marker.len();
    Some(command[begin..end].to_string())
}

fn parse_apply_patch(input: &str) -> Result<ParsedPatch, AppError> {
    let lines = input.lines().collect::<Vec<_>>();
    if lines.first().map(|line| line.trim()) != Some("*** Begin Patch") {
        return Err(AppError::BadRequest(
            "patch must start with *** Begin Patch".to_string(),
        ));
    }
    if lines.last().map(|line| line.trim()) != Some("*** End Patch") {
        return Err(AppError::BadRequest(
            "patch must end with *** End Patch".to_string(),
        ));
    }

    let mut index = 1;
    let mut hunks = Vec::new();
    while index + 1 < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut contents = Vec::new();
            while index + 1 < lines.len() && !lines[index].starts_with("*** ") {
                let content_line = lines[index].strip_prefix('+').ok_or_else(|| {
                    AppError::BadRequest(format!(
                        "add file hunk lines must start with '+': {}",
                        lines[index]
                    ))
                })?;
                contents.push(content_line.to_string());
                index += 1;
            }
            hunks.push(PatchHunk::AddFile {
                path: path.trim().to_string(),
                contents,
            });
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            hunks.push(PatchHunk::DeleteFile {
                path: path.trim().to_string(),
            });
            index += 1;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let mut move_path = None;
            if index + 1 < lines.len() {
                if let Some(target) = lines[index].strip_prefix("*** Move to: ") {
                    move_path = Some(target.trim().to_string());
                    index += 1;
                }
            }
            let mut chunks = Vec::new();
            let mut current = PatchChunk::default();
            while index + 1 < lines.len() && !is_patch_file_header(lines[index]) {
                let patch_line = lines[index];
                if patch_line == "@@" || patch_line.starts_with("@@ ") {
                    push_patch_chunk(&mut chunks, &mut current);
                    index += 1;
                    continue;
                }
                if patch_line == "*** End of File" {
                    index += 1;
                    continue;
                }
                let Some(prefix) = patch_line.chars().next() else {
                    index += 1;
                    continue;
                };
                let body = patch_line.get(1..).unwrap_or_default().to_string();
                match prefix {
                    ' ' => {
                        current.old_lines.push(body.clone());
                        current.new_lines.push(body);
                    }
                    '-' => current.old_lines.push(body),
                    '+' => current.new_lines.push(body),
                    _ => {
                        return Err(AppError::BadRequest(format!(
                            "invalid update hunk line: {patch_line}"
                        )));
                    }
                }
                index += 1;
            }
            push_patch_chunk(&mut chunks, &mut current);
            hunks.push(PatchHunk::UpdateFile {
                path: path.trim().to_string(),
                move_path,
                chunks,
            });
            continue;
        }

        return Err(unsupported_patch_line_error(line));
    }

    if hunks.is_empty() {
        return Err(AppError::BadRequest(
            "patch contains no file changes".to_string(),
        ));
    }

    Ok(ParsedPatch { hunks })
}

fn unsupported_patch_line_error(line: &str) -> AppError {
    AppError::BadRequest(format!(
        "unsupported patch line: {line}\n\nThis apply_patch tool does not accept standard unified diff headers such as '--- file' and '+++ file', and it does not accept Search/Replace prose blocks. Use this format instead:\n*** Begin Patch\n*** Update File: path/to/file\n@@\n-old line\n+new line\n*** End Patch\n\nFor adding a file:\n*** Begin Patch\n*** Add File: path/to/file\n+new line\n*** End Patch\n\nFor deleting a file:\n*** Begin Patch\n*** Delete File: path/to/file\n*** End Patch"
    ))
}

fn is_patch_file_header(line: &str) -> bool {
    line.starts_with("*** Add File: ")
        || line.starts_with("*** Delete File: ")
        || line.starts_with("*** Update File: ")
        || line == "*** End Patch"
}

fn push_patch_chunk(chunks: &mut Vec<PatchChunk>, current: &mut PatchChunk) {
    if current.old_lines.is_empty() && current.new_lines.is_empty() {
        return;
    }
    chunks.push(std::mem::take(current));
}

fn patch_affected_paths(parsed: &ParsedPatch, cwd: &Path) -> Result<Vec<PathBuf>, AppError> {
    let mut paths = Vec::new();
    for hunk in &parsed.hunks {
        match hunk {
            PatchHunk::AddFile { path, .. } | PatchHunk::DeleteFile { path } => {
                paths.push(resolve_patch_path(cwd, path)?);
            }
            PatchHunk::UpdateFile {
                path, move_path, ..
            } => {
                paths.push(resolve_patch_path(cwd, path)?);
                if let Some(move_path) = move_path {
                    paths.push(resolve_patch_path(cwd, move_path)?);
                }
            }
        }
    }

    let mut seen = BTreeSet::new();
    paths.retain(|path| seen.insert(normalize_display_path(path)));
    Ok(paths)
}

fn resolve_patch_path(cwd: &Path, raw: &str) -> Result<PathBuf, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(
            "patch path cannot be empty".to_string(),
        ));
    }
    let expanded = expand_home_path(trimmed);
    let path = PathBuf::from(expanded);
    let absolute = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    Ok(normalize_root_path(absolute))
}

fn apply_parsed_patch(parsed: &ParsedPatch, cwd: &Path) -> Result<PatchSummary, AppError> {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    for hunk in &parsed.hunks {
        match hunk {
            PatchHunk::AddFile { path, contents } => {
                let target = resolve_patch_path(cwd, path)?;
                if target.exists() {
                    return Err(AppError::Conflict(format!(
                        "file already exists: {}",
                        target.to_string_lossy()
                    )));
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&target, format!("{}\n", contents.join("\n")))?;
                added.push(path.clone());
            }
            PatchHunk::DeleteFile { path } => {
                let target = resolve_patch_path(cwd, path)?;
                if target.is_dir() {
                    return Err(AppError::BadRequest(format!(
                        "delete file target is a directory: {}",
                        target.to_string_lossy()
                    )));
                }
                fs::remove_file(&target)?;
                deleted.push(path.clone());
            }
            PatchHunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                let source = resolve_patch_path(cwd, path)?;
                if source.is_dir() {
                    return Err(AppError::BadRequest(format!(
                        "update file target is a directory: {}",
                        source.to_string_lossy()
                    )));
                }
                let original = fs::read_to_string(&source)?;
                let updated = apply_update_chunks(&original, chunks, &source)?;
                if let Some(move_path) = move_path {
                    let target = resolve_patch_path(cwd, move_path)?;
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&target, updated)?;
                    fs::remove_file(&source)?;
                    modified.push(move_path.clone());
                } else {
                    fs::write(&source, updated)?;
                    modified.push(path.clone());
                }
            }
        }
    }

    Ok(PatchSummary {
        added,
        modified,
        deleted,
    })
}

fn apply_update_chunks(
    original: &str,
    chunks: &[PatchChunk],
    path: &Path,
) -> Result<String, AppError> {
    let mut lines = original
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }

    let mut cursor = 0;
    for chunk in chunks {
        if chunk.old_lines.is_empty() {
            let insert_at = lines.len();
            lines.splice(insert_at..insert_at, chunk.new_lines.clone());
            cursor = insert_at + chunk.new_lines.len();
            continue;
        }

        let Some(found) = find_line_sequence(&lines, &chunk.old_lines, cursor)
            .or_else(|| find_line_sequence(&lines, &chunk.old_lines, 0))
        else {
            return Err(AppError::BadRequest(format!(
                "failed to find expected lines in {}:\n{}",
                path.to_string_lossy(),
                chunk.old_lines.join("\n")
            )));
        };
        let end = found + chunk.old_lines.len();
        lines.splice(found..end, chunk.new_lines.clone());
        cursor = found + chunk.new_lines.len();
    }

    Ok(format!("{}\n", lines.join("\n")))
}

fn find_line_sequence(lines: &[String], needle: &[String], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(lines.len()));
    }
    if needle.len() > lines.len() {
        return None;
    }
    (start..=lines.len() - needle.len())
        .find(|index| lines[*index..*index + needle.len()] == *needle)
}

fn patch_summary_text(summary: &PatchSummary) -> String {
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
    lines.join("\n")
}

fn truncate_preview(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out = input.chars().take(max_chars).collect::<String>();
    out.push_str("\n[preview truncated]");
    out
}

fn normalize_root_path(path: PathBuf) -> PathBuf {
    if path.exists() {
        match std::fs::canonicalize(&path) {
            Ok(value) => value,
            Err(_) => normalize_lexical_path(&path),
        }
    } else {
        normalize_lexical_path(&path)
    }
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn expand_home_path(token: &str) -> String {
    if let Some(rest) = token.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    if token == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    token.to_string()
}

fn normalize_command_token(token: &str) -> String {
    let cleaned = strip_matching_quotes(token);
    Path::new(cleaned)
        .file_name()
        .and_then(|item| item.to_str())
        .unwrap_or(cleaned)
        .to_ascii_lowercase()
}

fn is_comment_line(line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with("//")
        || line.starts_with("::")
        || line
            .get(0..3)
            .map(|prefix| prefix.eq_ignore_ascii_case("rem"))
            .unwrap_or(false)
}

fn split_shell_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in line.chars() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\'' | '"' => quote = Some(ch),
            ' ' | '\t' => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn is_path_like(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    if token.starts_with('-') {
        return false;
    }
    if token.contains("://") {
        return false;
    }
    if token.starts_with("~/")
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('\\')
        || token.starts_with('/')
        || token.contains('\\')
        || token.contains('/')
    {
        return true;
    }
    let bytes = token.as_bytes();
    bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn should_disable_output_truncation(program: &str, command_args: &[String]) -> bool {
    let normalized_program = normalize_command_token(program);
    let is_markdown_reader = matches!(
        normalized_program.as_str(),
        "cat" | "type" | "get-content" | "gc"
    );
    if !is_markdown_reader {
        return false;
    }

    command_args.iter().any(|arg| {
        let candidate = strip_matching_quotes(arg).trim();
        if candidate.is_empty() || candidate.starts_with('-') {
            return false;
        }

        let file_name = Path::new(candidate)
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or(candidate);
        let lowered = file_name.to_ascii_lowercase();
        lowered == "skill.md" || lowered.ends_with(".md") || lowered.ends_with(".markdown")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn command_tree_rule_can_trigger_confirmation() {
        let skills = SkillsConfig::default();
        let raw = "sh script.sh";
        let decision = evaluate_policy(
            &skills,
            "sh",
            &[String::from("script.sh")],
            raw,
            Path::new("scripts/script.sh"),
            Some("rm test.txt"),
        );

        match decision {
            PolicyDecision::Confirm(reason) => {
                assert!(reason.contains("confirm-rm"));
            }
            _ => panic!("expected confirm decision"),
        }
    }

    #[test]
    fn path_guard_blocks_outside_whitelist() {
        let mut skills = SkillsConfig::default();
        let base = std::env::current_dir().expect("cwd");
        let allowed = base.join("allowed-zone");
        let blocked = base.join("outside-zone").join("target.txt");

        skills.policy.path_guard.enabled = true;
        skills.policy.path_guard.whitelist_dirs = vec![allowed.to_string_lossy().to_string()];
        skills.policy.path_guard.on_violation = SkillPolicyAction::Deny;
        skills.policy.rules.clear();

        let raw = format!("python tool.py {}", blocked.to_string_lossy());
        let decision = evaluate_policy(
            &skills,
            "python",
            &[
                String::from("tool.py"),
                blocked.to_string_lossy().to_string(),
            ],
            &raw,
            &allowed.join("scripts").join("tool.py"),
            None,
        );

        match decision {
            PolicyDecision::Deny(reason) => {
                assert!(reason.contains("outside whitelist"));
            }
            _ => panic!("expected deny decision"),
        }
    }

    #[test]
    fn default_policy_allow_when_no_match() {
        let mut skills = SkillsConfig::default();
        skills.policy.rules.clear();
        skills.policy.path_guard.enabled = false;
        skills.policy.default_action = SkillPolicyAction::Allow;

        let raw = "python safe.py --help";
        let decision = evaluate_policy(
            &skills,
            "python",
            &[String::from("safe.py"), String::from("--help")],
            raw,
            Path::new("scripts/safe.py"),
            Some("print('ok')"),
        );

        match decision {
            PolicyDecision::Allow => {}
            _ => panic!("expected allow decision"),
        }
    }

    #[test]
    fn default_text_write_commands_require_confirmation() {
        let skills = SkillsConfig::default();
        let raw = "Set-Content -Path note.txt -Value hello";
        let decision = evaluate_policy(
            &skills,
            "Set-Content",
            &[
                String::from("-Path"),
                String::from("note.txt"),
                String::from("-Value"),
                String::from("hello"),
            ],
            raw,
            Path::new("scripts/write.ps1"),
            None,
        );

        match decision {
            PolicyDecision::Confirm(reason) => {
                assert!(reason.contains("confirm-set-content"));
            }
            _ => panic!("expected confirm decision"),
        }
    }

    #[test]
    fn policy_denied_text_explains_gateway_rejection() {
        let text = mcp_gateway_policy_denied_text("matched default policy");

        assert!(text.contains("MCP Gateway"));
        assert!(text.contains("已拒绝此命令"));
        assert!(text.contains("confirm"));
        assert!(text.contains("allow"));
    }

    #[test]
    fn shell_wrapper_chain_is_blocked_by_default_rules() {
        let skills = SkillsConfig::default();
        let raw = "bash -lc \"rm -rf /tmp/demo\"";
        let decision = evaluate_policy(
            &skills,
            "bash",
            &[String::from("-lc"), String::from("rm -rf /tmp/demo")],
            raw,
            Path::new("scripts/safe.sh"),
            None,
        );

        match decision {
            PolicyDecision::Deny(reason) => {
                assert!(reason.contains("deny-bash-lc"));
            }
            _ => panic!("expected deny decision"),
        }
    }

    #[tokio::test]
    async fn confirmation_wait_returns_approved_when_user_approves() {
        let service = SkillsService::new();
        let confirmation = match service
            .create_confirmation(
                "skill",
                "cmd",
                &[String::from("cmd")],
                "cmd",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };
        let confirmation_id = confirmation.id.clone();
        let service_for_approve = service.clone();
        let id_for_approve = confirmation_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = service_for_approve
                .approve_confirmation(&id_for_approve)
                .await;
        });

        let outcome = service
            .wait_for_confirmation_decision(
                &confirmation_id,
                Duration::from_secs(2),
                Duration::from_millis(10),
            )
            .await;
        match outcome {
            ConfirmationWaitOutcome::Approved => {}
            _ => panic!("expected approved outcome"),
        }
    }

    #[tokio::test]
    async fn create_confirmation_creates_distinct_requests_for_same_command() {
        let service = SkillsService::new();
        let first = match service
            .create_confirmation(
                "skill",
                "cmd-repeat",
                &[String::from("cmd-repeat")],
                "cmd-repeat",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };
        // 同指纹第二次调用 → Reused，复用同一条记录
        let second = match service
            .create_confirmation(
                "skill",
                "cmd-repeat",
                &[String::from("cmd-repeat")],
                "cmd-repeat",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Reused(c) => c,
            other => panic!("expected Reused, got {other:?}"),
        };
        assert_eq!(first.id, second.id);
        assert_eq!(first.status, ConfirmationStatus::Pending);
        assert_eq!(second.status, ConfirmationStatus::Pending);
    }

    #[tokio::test]
    async fn confirmation_wait_returns_rejected_when_user_rejects() {
        let service = SkillsService::new();
        let confirmation = match service
            .create_confirmation(
                "skill",
                "cmd-reject-wait",
                &[String::from("cmd-reject-wait")],
                "cmd-reject-wait",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };
        let confirmation_id = confirmation.id.clone();
        let service_for_reject = service.clone();
        let id_for_reject = confirmation_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = service_for_reject.reject_confirmation(&id_for_reject).await;
        });

        let outcome = service
            .wait_for_confirmation_decision(
                &confirmation_id,
                Duration::from_secs(2),
                Duration::from_millis(10),
            )
            .await;
        match outcome {
            ConfirmationWaitOutcome::Rejected => {}
            _ => panic!("expected rejected outcome"),
        }
    }

    #[tokio::test]
    async fn rejecting_one_confirmation_rejects_pending_duplicates() {
        let service = SkillsService::new();
        // 第一次：新建
        let first = match service
            .create_confirmation(
                "skill",
                "cmd-dup",
                &[String::from("cmd-dup")],
                "cmd-dup",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created for first, got {other:?}"),
        };
        // 第二次同指纹：复用
        let second = match service
            .create_confirmation(
                "skill",
                "cmd-dup",
                &[String::from("cmd-dup")],
                "cmd-dup",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Reused(c) => c,
            other => panic!("expected Reused for second, got {other:?}"),
        };
        assert_eq!(
            first.id, second.id,
            "same fingerprint should reuse the same entry"
        );
        let _ = service
            .reject_confirmation(&first.id)
            .await
            .expect("reject first");

        let second_outcome = service
            .wait_for_confirmation_decision(
                &second.id,
                Duration::from_secs(1),
                Duration::from_millis(10),
            )
            .await;
        match second_outcome {
            ConfirmationWaitOutcome::Rejected => {}
            _ => panic!("expected duplicate request to be rejected"),
        }
    }

    #[tokio::test]
    async fn concurrent_create_confirmation_requests_are_distinct() {
        let service = SkillsService::new();
        let s1 = service.clone();
        let s2 = service.clone();

        let one = tokio::spawn(async move {
            s1.create_confirmation(
                "skill",
                "cmd-dedupe",
                &[String::from("cmd-dedupe")],
                "cmd-dedupe",
                "need approval",
            )
            .await
        });
        let two = tokio::spawn(async move {
            s2.create_confirmation(
                "skill",
                "cmd-dedupe",
                &[String::from("cmd-dedupe")],
                "cmd-dedupe",
                "need approval",
            )
            .await
        });

        let first = one.await.expect("first join");
        let second = two.await.expect("second join");
        // 并发同指纹：一个 Created，另一个 Reused，二者复用同一条记录
        let first_id = match &first {
            CreateConfirmationResult::Created(c) | CreateConfirmationResult::Reused(c) => {
                c.id.clone()
            }
            CreateConfirmationResult::AlreadyTimedOut(id) => id.clone(),
        };
        let second_id = match &second {
            CreateConfirmationResult::Created(c) | CreateConfirmationResult::Reused(c) => {
                c.id.clone()
            }
            CreateConfirmationResult::AlreadyTimedOut(id) => id.clone(),
        };
        assert_eq!(
            first_id, second_id,
            "concurrent same-fingerprint calls should share one entry"
        );
    }

    #[tokio::test]
    async fn confirmation_wait_times_out_when_not_confirmed() {
        let service = SkillsService::new();
        let confirmation = match service
            .create_confirmation(
                "skill",
                "cmd-timeout",
                &[String::from("cmd-timeout")],
                "cmd-timeout",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };

        let outcome = service
            .wait_for_confirmation_decision(
                &confirmation.id,
                Duration::from_millis(40),
                Duration::from_millis(10),
            )
            .await;
        match outcome {
            ConfirmationWaitOutcome::TimedOut => {}
            _ => panic!("expected timed out outcome"),
        }
        let pending = service.list_pending_confirmations().await;
        assert!(pending.iter().all(|item| item.id != confirmation.id));
    }

    #[tokio::test]
    async fn rejected_confirmation_then_same_command_still_creates_new_request() {
        let service = SkillsService::new();
        let first = match service
            .create_confirmation(
                "skill",
                "cmd-reject",
                &[String::from("cmd-reject")],
                "cmd-reject",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };
        // 用户手动拒绝（timed_out=false）→ 允许重新发起
        let _ = service.reject_confirmation(&first.id).await;

        let reused = match service
            .create_confirmation(
                "skill",
                "cmd-reject",
                &[String::from("cmd-reject")],
                "cmd-reject",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created (new entry after user reject), got {other:?}"),
        };
        assert_ne!(reused.id, first.id);
        assert_eq!(reused.status, ConfirmationStatus::Pending);

        let outcome = service
            .wait_for_confirmation_decision(
                &first.id,
                Duration::from_secs(1),
                Duration::from_millis(10),
            )
            .await;
        match outcome {
            ConfirmationWaitOutcome::Rejected => {}
            _ => panic!("expected rejected outcome"),
        }
    }

    #[tokio::test]
    async fn approved_confirmation_then_same_command_still_creates_new_request() {
        let service = SkillsService::new();
        let first = match service
            .create_confirmation(
                "skill",
                "cmd-approve",
                &[String::from("cmd-approve")],
                "cmd-approve",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };
        let _ = service.approve_confirmation(&first.id).await;

        let reused = match service
            .create_confirmation(
                "skill",
                "cmd-approve",
                &[String::from("cmd-approve")],
                "cmd-approve",
                "need approval",
            )
            .await
        {
            CreateConfirmationResult::Created(c) => c,
            other => panic!("expected Created (new entry after approve), got {other:?}"),
        };
        assert_ne!(reused.id, first.id);
        assert_eq!(reused.status, ConfirmationStatus::Pending);

        let outcome = service
            .wait_for_confirmation_decision(
                &first.id,
                Duration::from_secs(1),
                Duration::from_millis(10),
            )
            .await;
        match outcome {
            ConfirmationWaitOutcome::Approved => {}
            _ => panic!("expected approved outcome"),
        }
    }

    #[test]
    fn chained_remove_item_requires_confirmation() {
        let skills = SkillsConfig::default();
        let raw = "Set-Location 'D:/Code_Save/demo'; Remove-Item -Force package-lock.json -ErrorAction SilentlyContinue; npm install";
        let tokens = split_shell_tokens(raw);
        let decision = evaluate_policy(
            &skills,
            &tokens[0],
            &tokens[1..],
            raw,
            Path::new("scripts/safe.ps1"),
            None,
        );

        match decision {
            PolicyDecision::Confirm(reason) => {
                assert!(reason.contains("confirm-remove-item"));
            }
            _ => panic!("expected confirm decision"),
        }
    }

    #[test]
    fn markdown_read_commands_disable_output_truncation() {
        assert!(should_disable_output_truncation(
            "Get-Content",
            &[
                String::from("-Raw"),
                String::from("D:/skills/demo/SKILL.md")
            ]
        ));
        assert!(should_disable_output_truncation(
            "cat",
            &[String::from("./references/weather_info.md")]
        ));
        assert!(!should_disable_output_truncation(
            "python",
            &[String::from("scripts/tool.py")]
        ));
    }

    #[test]
    fn command_output_text_formats_stdout_and_stderr() {
        assert_eq!(command_output_text("hello\n", ""), "hello");
        assert_eq!(command_output_text("", "warn\n"), "warn");
        assert_eq!(
            command_output_text("ok\n", "warn\n"),
            "ok\n\n[stderr]\nwarn"
        );
    }

    #[test]
    fn command_failure_text_includes_exit_code() {
        assert_eq!(
            command_failure_text(2, "", ""),
            "command finished with non-zero exit code (2) and no output"
        );
        assert_eq!(
            command_failure_text(2, "oops\n", ""),
            "command finished with non-zero exit code (2).\noops"
        );
    }

    #[test]
    fn command_timeout_text_includes_last_output() {
        assert_eq!(
            command_timeout_text(60_000, "", ""),
            "command timed out after 60000ms and produced no output"
        );
        assert_eq!(
            command_timeout_text(60_000, "waiting for input\n", ""),
            "command timed out after 60000ms.\nLast output:\nwaiting for input"
        );
    }

    #[tokio::test]
    async fn execute_skill_command_timeout_returns_last_output() {
        let command_text = if cfg!(target_os = "windows") {
            "Write-Output 'waiting for input'; Start-Sleep -Seconds 5"
        } else {
            "printf 'waiting for input\\n'; sleep 5"
        };
        let timeout_ms = if cfg!(target_os = "windows") {
            800
        } else {
            250
        };
        let (runner, runner_args) = shell_command_for_current_os(command_text);
        let mut command = Command::new(&runner);
        command
            .args(&runner_args)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        configure_skill_command(&mut command);

        let error = execute_skill_command(&mut command, timeout_ms, 4096, false)
            .await
            .expect_err("command should time out");

        match error {
            AppError::Upstream(message) => {
                assert!(message.contains(&format!("command timed out after {timeout_ms}ms")));
                assert!(message.contains("Last output:"));
                assert!(message.contains("waiting for input"));
            }
            other => panic!("expected upstream timeout error, got {other:?}"),
        }
    }

    #[test]
    fn discover_skills_sync_finds_root_and_nested_skill_directories() {
        let sandbox = std::env::temp_dir().join(format!("gateway-skills-{}", Uuid::new_v4()));
        let root_skill_dir = sandbox.join("root-skill");
        let nested_skill_dir = root_skill_dir.join("nested").join("child-skill");
        std::fs::create_dir_all(&nested_skill_dir).expect("create test directories");
        std::fs::write(
            root_skill_dir.join("SKILL.md"),
            "---\nname: root-skill\n---\n# Root\n",
        )
        .expect("write root SKILL.md");
        std::fs::write(
            nested_skill_dir.join("SKILL.md"),
            "---\nname: child-skill\n---\n# Child\n",
        )
        .expect("write nested SKILL.md");

        let roots = vec![root_skill_dir.to_string_lossy().to_string()];
        let discovered = discover_skills_sync(&roots).expect("discover skills");
        let names = discovered
            .iter()
            .map(|skill| skill.skill.clone())
            .collect::<HashSet<_>>();

        assert!(names.contains("root-skill"));
        assert!(names.contains("child-skill"));

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn parse_frontmatter_fields_reads_yaml_mapping_and_metadata() {
        let sandbox = std::env::temp_dir().join(format!("gateway-frontmatter-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let skill_md = sandbox.join("SKILL.md");
        std::fs::write(
            &skill_md,
            "---\nname: demo-skill\ndescription: Demo description\nmetadata:\n  tags:\n    - remotion\n    - video\n  image:\n    format: png\n---\n# Demo\n",
        )
        .expect("write SKILL.md");

        let parsed = parse_frontmatter_fields(&skill_md).expect("parse frontmatter");
        assert_eq!(parsed.name, "demo-skill");
        assert_eq!(parsed.description, "Demo description");
        assert!(parsed
            .metadata
            .contains("\"tags\":[\"remotion\",\"video\"]"));
        assert!(parsed.metadata.contains("\"image\":{\"format\":\"png\"}"));
        assert!(parsed.block.contains("metadata:"));

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn tool_definitions_use_metadata_name_and_cmd_schema() {
        let discovered = vec![
            DiscoveredSkill {
                skill: "alpha".to_string(),
                frontmatter_name: "Alpha Skill".to_string(),
                description: "A".to_string(),
                frontmatter_metadata: r#"{"tags":["alpha"]}"#.to_string(),
                frontmatter_block:
                    "name: Alpha Skill\ndescription: A\nmetadata:\n  tags:\n    - alpha".to_string(),
                root: PathBuf::from("C:/skills"),
                path: PathBuf::from("C:/skills/alpha"),
                has_scripts: true,
            },
            DiscoveredSkill {
                skill: "beta".to_string(),
                frontmatter_name: "Beta Skill".to_string(),
                description: "B".to_string(),
                frontmatter_metadata: "none".to_string(),
                frontmatter_block: "name: Beta Skill\ndescription: B".to_string(),
                root: PathBuf::from("C:/skills"),
                path: PathBuf::from("C:/skills/beta"),
                has_scripts: false,
            },
        ];
        let tools = tool_definitions(&discovered);
        let shell_tool = tools
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("name").and_then(Value::as_str) == Some("shell_command"))
            })
            .expect("shell command tool exists");
        let shell_description = shell_tool
            .get("description")
            .and_then(Value::as_str)
            .expect("shell description");
        assert!(shell_description.contains("no separate SKILL.md"));
        assert!(shell_description.contains("rg --files"));
        assert!(shell_description.contains("Get-Content -Raw"));

        let patch_tool = tools
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("name").and_then(Value::as_str) == Some("apply_patch"))
            })
            .expect("apply patch tool exists");
        let patch_description = patch_tool
            .get("description")
            .and_then(Value::as_str)
            .expect("patch description");
        assert!(patch_description.contains("*** Update File:"));
        assert!(patch_description.contains("does not accept standard unified diff"));
        assert!(patch_description.contains("-<h1>Old title</h1>"));

        let alpha_tool = tools
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("name").and_then(Value::as_str) == Some("alpha_skill"))
            })
            .expect("alpha skill tool exists");
        let cmd_schema = alpha_tool
            .get("inputSchema")
            .and_then(|schema| schema.get("properties"))
            .and_then(|props| props.get("cmd"))
            .expect("cmd schema is present");
        assert_eq!(
            cmd_schema.get("type").and_then(Value::as_str),
            Some("string")
        );

        let description = alpha_tool
            .get("description")
            .and_then(Value::as_str)
            .expect("tool description");
        assert!(description.contains("Current OS:"));
        assert!(description.contains("Current datetime:"));
        assert!(description.contains("SKILL.md"));
        assert!(description.contains("Front matter summary:"));
        assert!(description.contains("name: Alpha Skill"));
        assert!(description.contains("metadata: {\"tags\":[\"alpha\"]}"));
        assert!(description.contains("Front matter raw (YAML):"));
        assert!(description.contains("metadata:"));

        let names = tools
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|item| item.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["shell_command", "apply_patch", "alpha_skill", "beta_skill"]
        );
    }

    #[test]
    fn apply_patch_updates_adds_and_deletes_files() {
        let sandbox = std::env::temp_dir().join(format!("gateway-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let update_path = sandbox.join("update.txt");
        let delete_path = sandbox.join("delete.txt");
        std::fs::write(&update_path, "alpha\nbeta\n").expect("write update");
        std::fs::write(&delete_path, "remove me\n").expect("write delete");

        let patch = format!(
            "*** Begin Patch\n*** Update File: update.txt\n@@\n alpha\n-beta\n+gamma\n*** Add File: added.txt\n+new file\n*** Delete File: delete.txt\n*** End Patch"
        );
        let parsed = parse_apply_patch(&patch).expect("parse patch");
        let affected = patch_affected_paths(&parsed, &sandbox).expect("affected paths");
        assert_eq!(affected.len(), 3);

        let summary = apply_parsed_patch(&parsed, &sandbox).expect("apply patch");
        assert_eq!(summary.added, vec!["added.txt"]);
        assert_eq!(summary.modified, vec!["update.txt"]);
        assert_eq!(summary.deleted, vec!["delete.txt"]);
        assert_eq!(
            std::fs::read_to_string(sandbox.join("update.txt")).expect("read update"),
            "alpha\ngamma\n"
        );
        assert_eq!(
            std::fs::read_to_string(sandbox.join("added.txt")).expect("read added"),
            "new file\n"
        );
        assert!(!delete_path.exists());

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn apply_patch_rejects_unified_diff_with_format_hint() {
        let patch = "*** Begin Patch\n--- index.html\n+++ index.html\n@@ -1 +1 @@\n-old\n+new\n*** End Patch";
        let error = parse_apply_patch(patch).expect_err("unified diff should be rejected");
        match error {
            AppError::BadRequest(message) => {
                assert!(message.contains("does not accept standard unified diff"));
                assert!(message.contains("*** Update File: path/to/file"));
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }
}
