use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use gateway_core::{
    assign_child_to_gateway_job, wrap_windows_powershell_command_for_utf8, AppError,
    BuiltinToolsConfig, ErrorCode, GatewayConfig, SkillCommandRule, SkillPolicyAction,
    SkillsConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{Notify, RwLock};
use utoipa::ToSchema;
use uuid::Uuid;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy)]
struct BundledTool {
    file_name: &'static str,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/bundled_tools.rs"));

const BUILTIN_SHELL_COMMAND_SKILL_MD: &str =
    include_str!("../builtin-skills/shell_command/SKILL.md");
const BUILTIN_APPLY_PATCH_SKILL_MD: &str = include_str!("../builtin-skills/apply_patch/SKILL.md");
const BUILTIN_MULTI_EDIT_FILE_SKILL_MD: &str =
    include_str!("../builtin-skills/multi_edit_file/SKILL.md");
const BUILTIN_TASK_PLANNING_SKILL_MD: &str =
    include_str!("../builtin-skills/task-planning/SKILL.md");
const BUILTIN_CHROME_CDP_SKILL_MD: &str = include_str!("../builtin-skills/chrome-cdp/SKILL.md");
const BUILTIN_CHROME_CDP_MJS: &str = include_str!("../builtin-skills/chrome-cdp/scripts/cdp.mjs");
const BUILTIN_CHAT_PLUS_ADAPTER_DEBUGGER_SKILL_MD: &str =
    include_str!("../builtin-skills/chat-plus-adapter-debugger/SKILL.md");
const BUILTIN_CHROME_CDP_DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[cfg(target_os = "windows")]
fn configure_skill_command(command: &mut Command) {
    // Keep skill scripts headless on Windows to avoid flashing cmd/powershell windows.
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn configure_skill_command(_command: &mut Command) {}

fn configure_bundled_tool_path(command: &mut Command) {
    let entries = bundled_tool_path_entries();
    if entries.is_empty() {
        return;
    }

    let current_path = env::var_os("PATH");
    if let Some(path) = prepend_path_entries(&entries, current_path.as_deref()) {
        command.env("PATH", path);
    }
}

fn bundled_tool_path_entries() -> Vec<PathBuf> {
    static ENTRIES: OnceLock<Vec<PathBuf>> = OnceLock::new();
    ENTRIES
        .get_or_init(|| {
            BUNDLED_RIPGREP
                .and_then(materialize_bundled_tool)
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .into_iter()
                .collect()
        })
        .clone()
}

fn prepend_path_entries(
    entries: &[PathBuf],
    current_path: Option<&OsStr>,
) -> Option<std::ffi::OsString> {
    let mut paths = entries.to_vec();
    if let Some(current_path) = current_path {
        paths.extend(env::split_paths(current_path));
    }
    env::join_paths(paths).ok()
}

fn materialize_bundled_tool(tool: BundledTool) -> Option<PathBuf> {
    let cache_root = dirs::cache_dir().unwrap_or_else(env::temp_dir);
    let tool_dir = cache_root.join("mcp-gateway").join("tools").join("ripgrep");
    let tool_path = tool_dir.join(tool.file_name);

    let should_write = fs::read(&tool_path)
        .map(|existing| existing != tool.bytes)
        .unwrap_or(true);
    if should_write {
        fs::create_dir_all(&tool_dir).ok()?;
        fs::write(&tool_path, tool.bytes).ok()?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&tool_path).ok()?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&tool_path, permissions).ok()?;
    }

    Some(tool_path)
}

#[derive(Clone, Default)]
pub struct SkillsService {
    confirmations: Arc<RwLock<HashMap<String, ConfirmationEntry>>>,
    discovery_cache: Arc<RwLock<Option<SkillDiscoveryCache>>>,
    events: Arc<RwLock<SkillEventStore>>,
    planning: Arc<RwLock<HashMap<String, PlanningState>>>,
}

#[derive(Debug, Default)]
struct SkillEventStore {
    next_seq: u64,
    events: VecDeque<SkillToolEvent>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
enum PlanItemStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanItem {
    step: String,
    status: PlanItemStatus,
}

#[derive(Debug, Clone)]
struct PlanningState {
    planning_id: String,
    plan: Vec<PlanItem>,
    explanation: Option<String>,
    consecutive_shell_commands: u32,
    consecutive_apply_patch_failures: u32,
    consecutive_multi_edit_file_failures: u32,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct PlanningSuccessHints {
    planning_reminder: Option<String>,
    shell_command_reminder: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanningLookupError {
    Unknown,
    Ambiguous,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskPlanningAction {
    Update,
    SetStatus,
    Clear,
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
    exec: String,
    #[serde(default)]
    skill_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinShellArgs {
    exec: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    skill_token: Option<String>,
    #[serde(default)]
    planning_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyPatchArgs {
    patch: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    skill_token: Option<String>,
    #[serde(default)]
    planning_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MultiEditFileArgs {
    path: String,
    edits: Vec<MultiEditFileEdit>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    skill_token: Option<String>,
    #[serde(default)]
    planning_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskPlanningArgs {
    #[serde(default)]
    exec: Option<String>,
    #[serde(default)]
    action: Option<TaskPlanningAction>,
    #[serde(default)]
    explanation: Option<String>,
    #[serde(default)]
    plan: Vec<PlanItem>,
    #[serde(default)]
    planning_id: Option<String>,
    #[serde(default)]
    item: Option<usize>,
    #[serde(default)]
    status: Option<PlanItemStatus>,
    #[serde(default)]
    skill_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MultiEditFileEdit {
    #[serde(alias = "oldString")]
    old_string: String,
    #[serde(alias = "newString")]
    new_string: String,
    #[serde(default, alias = "replaceAll")]
    replace_all: bool,
    #[serde(default, alias = "startLine")]
    start_line: Option<usize>,
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
    MultiEditFile,
    TaskPlanning,
    ChromeCdp,
    ChatPlusAdapterDebugger,
}

#[derive(Debug, Clone)]
struct ConfirmationMetadata {
    kind: String,
    cwd: String,
    affected_paths: Vec<String>,
    preview: String,
}

#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
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
    change_context: Option<String>,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    is_end_of_file: bool,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillToolEvent {
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub call_id: String,
    pub tool: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub affected_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<AppliedPatchDelta>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Default)]
struct SkillToolEventData {
    cwd: Option<String>,
    preview: Option<String>,
    text: Option<String>,
    status: Option<String>,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
    affected_paths: Vec<String>,
    changes: Option<Value>,
    delta: Option<AppliedPatchDelta>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppliedPatchDelta {
    changes: Vec<AppliedPatchChange>,
    exact: bool,
}

impl Default for AppliedPatchDelta {
    fn default() -> Self {
        Self {
            changes: Vec::new(),
            exact: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AppliedPatchChange {
    Add {
        path: String,
        content: String,
        overwritten_content: Option<String>,
    },
    Delete {
        path: String,
        content: Option<String>,
    },
    Update {
        path: String,
        move_path: Option<String>,
        old_content: String,
        new_content: String,
        overwritten_move_content: Option<String>,
    },
}

#[derive(Debug)]
struct ApplyPatchFailure {
    message: String,
    delta: AppliedPatchDelta,
}

#[derive(Debug)]
struct ApplyPatchOutcome {
    summary: PatchSummary,
    delta: AppliedPatchDelta,
    warnings: Vec<String>,
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

#[derive(Clone)]
struct SkillStreamEmitter {
    service: SkillsService,
    call_id: String,
    tool: String,
    kind: &'static str,
}

impl SkillStreamEmitter {
    async fn emit(&self, text: String) {
        if text.is_empty() {
            return;
        }
        self.service
            .record_tool_event_data(
                &self.call_id,
                &self.tool,
                self.kind,
                SkillToolEventData {
                    text: Some(text),
                    ..SkillToolEventData::default()
                },
            )
            .await;
    }
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
    const MAX_TOOL_EVENTS: usize = 500;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_skills_server(&self, config: &GatewayConfig, server_name: &str) -> bool {
        config.skills.server_name == server_name || config.skills.builtin_server_name == server_name
    }

    pub async fn handle_mcp_request(
        &self,
        config: &GatewayConfig,
        request: Value,
        session_id: Option<&str>,
        server_name: &str,
    ) -> Value {
        let Some(object) = request.as_object() else {
            return jsonrpc_error(Value::Null, -32600, "invalid request payload", None);
        };

        let id = object.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return jsonrpc_error(id, -32600, "missing jsonrpc method", None);
        };

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
                let is_builtin = server_name == config.skills.builtin_server_name;
                let (discovered, summaries) = if is_builtin {
                    (Vec::new(), Vec::new())
                } else {
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
                    (discovered, summaries)
                };
                let tools = if is_builtin {
                    Value::Array(builtin_tool_definitions(
                        std::env::consts::OS,
                        &Utc::now().to_rfc3339(),
                        &config.skills.builtin_tools,
                    ))
                } else {
                    external_skill_tool_definitions(&discovered)
                };
                jsonrpc_result(
                    id,
                    json!({
                        "tools": tools,
                        "skills": summaries
                    }),
                )
            }
            "tools/call" => {
                let is_builtin = server_name == config.skills.builtin_server_name;
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

                let planning_scope = planning_scope_key(session_id);
                let result = match self
                    .execute_tool_call(config, tool_params, &planning_scope, is_builtin)
                    .await
                {
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
        let discovered = self.discover_skills(&config.skills).await?;
        let mut summaries = summarize_builtin_skills(&config.skills.builtin_tools);
        summaries.extend(summarize_discovered_skills(&discovered));
        Ok(summaries)
    }

    pub async fn list_tool_events(&self, after: Option<u64>) -> Vec<SkillToolEvent> {
        let after = after.unwrap_or(0);
        let guard = self.events.read().await;
        guard
            .events
            .iter()
            .filter(|event| event.seq > after)
            .cloned()
            .collect()
    }

    async fn record_tool_event(&self, mut event: SkillToolEvent) {
        let mut guard = self.events.write().await;
        guard.next_seq = guard.next_seq.saturating_add(1);
        event.seq = guard.next_seq;
        guard.events.push_back(event);
        while guard.events.len() > Self::MAX_TOOL_EVENTS {
            guard.events.pop_front();
        }
    }

    async fn record_tool_event_data(
        &self,
        call_id: &str,
        tool: &str,
        kind: &str,
        data: SkillToolEventData,
    ) {
        self.record_tool_event(SkillToolEvent {
            seq: 0,
            timestamp: Utc::now(),
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            kind: kind.to_string(),
            cwd: data.cwd,
            preview: data.preview,
            text: data.text,
            status: data.status,
            exit_code: data.exit_code,
            duration_ms: data.duration_ms,
            affected_paths: data.affected_paths,
            changes: data.changes,
            delta: data.delta,
            warnings: data.warnings,
        })
        .await;
    }

    async fn execute_tool_call(
        &self,
        config: &GatewayConfig,
        params: ToolCallParams,
        planning_scope: &str,
        is_builtin_endpoint: bool,
    ) -> Result<ToolResult, AppError> {
        if let Some(tool) = BuiltinTool::from_name(&params.name) {
            if is_builtin_endpoint {
                return self
                    .execute_builtin_tool(config, tool, params.arguments, planning_scope)
                    .await;
            }
            return Err(AppError::BadRequest(format!(
                "built-in tool {} is only available on the built-in skills endpoint",
                params.name
            )));
        }

        if is_builtin_endpoint {
            return Err(AppError::BadRequest(format!(
                "unknown tool name: {}",
                params.name
            )));
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

        state.consecutive_apply_patch_failures = 0;
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
            Some(format!(
                "Current plan item #{idx} is in_progress: \"{}\". If this tool completed that item, call task-planning with {{\"action\":\"set_status\",\"planningId\":\"{}\",\"status\":\"completed\"}}; otherwise keep reusing the same planningId.",
                item.step, state.planning_id
            ))
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

        let (tool_name, count) = match tool {
            BuiltinTool::ApplyPatch => {
                state.consecutive_apply_patch_failures =
                    state.consecutive_apply_patch_failures.saturating_add(1);
                state.consecutive_multi_edit_file_failures = 0;
                (
                    BuiltinTool::ApplyPatch.name(),
                    state.consecutive_apply_patch_failures,
                )
            }
            BuiltinTool::MultiEditFile => {
                state.consecutive_multi_edit_file_failures =
                    state.consecutive_multi_edit_file_failures.saturating_add(1);
                state.consecutive_apply_patch_failures = 0;
                (
                    BuiltinTool::MultiEditFile.name(),
                    state.consecutive_multi_edit_file_failures,
                )
            }
            _ => return None,
        };

        if count >= 3 {
            Some(format!(
                "{tool_name} has failed {count} times in a row for this planningId. Consider switching write strategy: use the other edit tool, simplify the patch, inspect the exact file content first, or use shell_command with a focused script when structured edits keep failing."
            ))
        } else {
            None
        }
    }

    async fn execute_builtin_tool(
        &self,
        config: &GatewayConfig,
        tool: BuiltinTool,
        arguments: Value,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
        if !builtin_tools(&config.skills.builtin_tools).contains(&tool) {
            return Err(AppError::BadRequest(format!(
                "builtin tool {} is disabled by configuration",
                tool.name()
            )));
        }
        match tool {
            BuiltinTool::ShellCommand => {
                let args = decode_tool_args::<BuiltinShellArgs>(&arguments)?;
                self.handle_builtin_shell_command(config, args, planning_scope)
                    .await
            }
            BuiltinTool::ApplyPatch => {
                let args = decode_tool_args::<ApplyPatchArgs>(&arguments)?;
                self.handle_builtin_apply_patch(config, args, planning_scope)
                    .await
            }
            BuiltinTool::MultiEditFile => {
                let args = decode_tool_args::<MultiEditFileArgs>(&arguments)?;
                self.handle_builtin_multi_edit_file(config, args, planning_scope)
                    .await
            }
            BuiltinTool::TaskPlanning => {
                let args = decode_tool_args::<TaskPlanningArgs>(&arguments)?;
                self.handle_builtin_task_planning(args, planning_scope)
                    .await
            }
            BuiltinTool::ChromeCdp => {
                let args = decode_tool_args::<BuiltinShellArgs>(&arguments)?;
                self.handle_builtin_chrome_cdp(config, args, planning_scope)
                    .await
            }
            BuiltinTool::ChatPlusAdapterDebugger => {
                let args = decode_tool_args::<BuiltinShellArgs>(&arguments)?;
                self.handle_builtin_chat_plus_adapter_debugger(config, args, planning_scope)
                    .await
            }
        }
    }

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
            return Ok(builtin_skill_doc_result(
                tool,
                &command_preview,
                matched_path,
                builtin_skill_token(tool),
                Self::planning_enabled(config),
            ));
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

        if let Some(patch) = extract_apply_patch_from_shell_command(&command_preview) {
            if !config.skills.builtin_tools.apply_patch {
                return Err(AppError::BadRequest(
                    "builtin tool apply_patch is disabled by configuration; cannot execute a patch from shell_command"
                        .to_string(),
                ));
            }
            return self
                .execute_apply_patch_text(
                    config,
                    patch,
                    &cwd,
                    &command_preview,
                    &call_id,
                    planning_scope,
                    args.planning_id.as_deref(),
                )
                .await;
        }

        let tokens = split_shell_tokens(&command_preview);
        if tokens.is_empty() {
            return Err(AppError::BadRequest("exec cannot be empty".to_string()));
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
                        "policyAction": "deny",
                        "policyHelp": mcp_gateway_policy_denied_help()
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
        self.record_tool_event_data(
            &call_id,
            BuiltinTool::ShellCommand.name(),
            "finished",
            SkillToolEventData {
                status: Some(if output.status.success() {
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

        if output.status.success() {
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

    async fn handle_builtin_apply_patch(
        &self,
        config: &GatewayConfig,
        args: ApplyPatchArgs,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
        let call_id = Uuid::new_v4().to_string();
        if let Some(result) = validate_skill_token_result(
            BuiltinTool::ApplyPatch.name(),
            &builtin_skill_token(BuiltinTool::ApplyPatch),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        if let Some(result) = self
            .check_planning_gate(
                config,
                planning_scope,
                BuiltinTool::ApplyPatch,
                args.planning_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        let cwd =
            match resolve_builtin_cwd(BuiltinTool::ApplyPatch, &config.skills, args.cwd.as_deref())
            {
                Ok(cwd) => cwd,
                Err(result) => return Ok(result),
            };
        self.execute_apply_patch_text(
            config,
            args.patch,
            &cwd,
            "apply_patch",
            &call_id,
            planning_scope,
            args.planning_id.as_deref(),
        )
        .await
    }

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

        let target = resolve_patch_path(&cwd, &args.path)?;
        let affected_paths = vec![target.clone()];
        let preview = multi_edit_preview(&args);
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
                changes: Some(multi_edit_preview_changes(&args)),
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
                        "policyHelp": mcp_gateway_policy_denied_help(),
                        "affectedPaths": affected_paths.iter().map(|path| normalize_display_path(path)).collect::<Vec<_>>()
                    }),
                ));
            }
            PolicyDecision::Confirm(reason) => {
                let metadata = ConfirmationMetadata {
                    kind: "edit".to_string(),
                    cwd: normalize_display_path(&cwd),
                    affected_paths: affected_paths
                        .iter()
                        .map(|path| normalize_display_path(path))
                        .collect(),
                    preview: truncate_preview(&preview, 4000),
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
                        return Ok(confirmation_rejected_result(&confirmation_id, false));
                    }
                    ConfirmationWaitOutcome::TimedOut => {
                        return Ok(confirmation_rejected_result(&confirmation_id, true));
                    }
                }
            }
            PolicyDecision::Allow => {}
        }

        match apply_multi_edit_file(&target, &args) {
            Ok(outcome) => {
                let text = patch_summary_text(&outcome.summary);
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
                        "modified": outcome.summary.modified,
                        "delta": patch_delta_for_model(&outcome.delta),
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
                        "delta": patch_delta_for_model(&failure.delta)
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

    #[allow(clippy::too_many_arguments)]
    async fn execute_apply_patch_text(
        &self,
        config: &GatewayConfig,
        patch: String,
        cwd: &Path,
        raw_command: &str,
        call_id: &str,
        planning_scope: &str,
        planning_id: Option<&str>,
    ) -> Result<ToolResult, AppError> {
        let patch_preview = patch.trim().to_string();
        if patch_preview.is_empty() {
            return Err(AppError::BadRequest("patch cannot be empty".to_string()));
        }

        let parsed = parse_apply_patch(&patch_preview)?;
        let affected_paths = patch_affected_paths(&parsed, cwd)?;
        self.record_tool_event_data(
            call_id,
            BuiltinTool::ApplyPatch.name(),
            "patchPreview",
            SkillToolEventData {
                cwd: Some(normalize_display_path(cwd)),
                preview: Some(truncate_preview(&patch_preview, 4000)),
                affected_paths: affected_paths
                    .iter()
                    .map(|path| normalize_display_path(path))
                    .collect(),
                changes: Some(patch_preview_changes(&parsed)),
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
                        "tool": BuiltinTool::ApplyPatch.name(),
                        "cwd": normalize_display_path(cwd),
                        "policyAction": "deny",
                        "policyHelp": mcp_gateway_policy_denied_help(),
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

        match apply_parsed_patch(&parsed, cwd) {
            Ok(outcome) => {
                let text = patch_summary_text(&outcome.summary);
                self.record_tool_event_data(
                    call_id,
                    BuiltinTool::ApplyPatch.name(),
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
                        "tool": BuiltinTool::ApplyPatch.name(),
                        "cwd": normalize_display_path(cwd),
                        "added": outcome.summary.added,
                        "modified": outcome.summary.modified,
                        "deleted": outcome.summary.deleted,
                        "delta": patch_delta_for_model(&outcome.delta),
                        "warnings": outcome.warnings
                    }),
                    self.planning_success_hints(
                        config,
                        planning_scope,
                        planning_id,
                        BuiltinTool::ApplyPatch,
                        None,
                    )
                    .await,
                ))
            }
            Err(failure) => {
                self.record_tool_event_data(
                    call_id,
                    BuiltinTool::ApplyPatch.name(),
                    "finished",
                    SkillToolEventData {
                        status: Some("failed".to_string()),
                        delta: Some(failure.delta.clone()),
                        ..SkillToolEventData::default()
                    },
                )
                .await;
                Ok(tool_error_with_edit_failure_reminder(
                    format!(
                        "{}\nCommitted patch delta exact: {}\nCommitted changes: {}",
                        failure.message,
                        failure.delta.exact,
                        failure.delta.changes.len()
                    ),
                    json!({
                        "status": "failed",
                        "tool": BuiltinTool::ApplyPatch.name(),
                        "cwd": normalize_display_path(cwd),
                        "message": failure.message,
                        "delta": patch_delta_for_model(&failure.delta)
                    }),
                    self.planning_edit_failure_reminder(
                        config,
                        planning_scope,
                        planning_id,
                        BuiltinTool::ApplyPatch,
                    )
                    .await,
                ))
            }
        }
    }

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
                        consecutive_apply_patch_failures: 0,
                        consecutive_multi_edit_file_failures: 0,
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
                state.consecutive_apply_patch_failures = 0;
                state.consecutive_multi_edit_file_failures = 0;
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

    async fn handle_builtin_chrome_cdp(
        &self,
        config: &GatewayConfig,
        args: BuiltinShellArgs,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
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
            BuiltinTool::ChromeCdp.name(),
            &builtin_skill_token(BuiltinTool::ChromeCdp),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        if let Some(result) = self
            .check_planning_gate(
                config,
                planning_scope,
                BuiltinTool::ChromeCdp,
                args.planning_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        self.execute_builtin_chrome_cdp_command(
            config,
            BuiltinTool::ChromeCdp.name(),
            &command_preview,
            &command_preview,
            args.timeout_ms,
            planning_scope,
            args.planning_id.as_deref(),
            BuiltinTool::ChromeCdp,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_builtin_chrome_cdp_command(
        &self,
        config: &GatewayConfig,
        tool_name: &str,
        command_preview: &str,
        structured_command: &str,
        timeout_ms: Option<u64>,
        planning_scope: &str,
        planning_id: Option<&str>,
        planning_tool: BuiltinTool,
    ) -> Result<ToolResult, AppError> {
        let cdp_args = parse_builtin_chrome_cdp_args(command_preview)?;
        let cdp_script = materialize_builtin_chrome_cdp_script()?;
        let cdp_runtime_dir = builtin_chrome_cdp_runtime_dir()?;
        let cdp_user_data_dir = builtin_chrome_cdp_user_data_dir()?;
        let effective_user_data_dir = std::env::var_os("CDP_USER_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| cdp_user_data_dir.clone());
        let effective_profile_mode =
            std::env::var("CDP_PROFILE_MODE").unwrap_or_else(|_| "persistent".to_string());
        let effective_browser_mode =
            std::env::var("CDP_BROWSER_MODE").unwrap_or_else(|_| "launch".to_string());
        let timeout_ms = timeout_ms
            .unwrap_or_else(|| {
                config
                    .skills
                    .execution
                    .timeout_ms
                    .max(BUILTIN_CHROME_CDP_DEFAULT_TIMEOUT_MS)
            })
            .max(1000);
        let max_output_bytes = config.skills.execution.max_output_bytes.max(1024);

        let started = Instant::now();
        let mut command = Command::new(node_command());
        command
            .arg(&cdp_script)
            .args(&cdp_args)
            .env("CDP_RUNTIME_DIR", &cdp_runtime_dir)
            .env("CDP_USER_DATA_DIR", &effective_user_data_dir)
            .env("CDP_PROFILE_MODE", &effective_profile_mode)
            .env("CDP_BROWSER_MODE", &effective_browser_mode)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        configure_bundled_tool_path(&mut command);
        configure_skill_command(&mut command);

        let output = execute_skill_command(
            &mut command,
            timeout_ms,
            max_output_bytes,
            false,
            None,
            None,
        )
        .await?;
        let duration_ms = started.elapsed().as_millis() as u64;
        let stdout = output.stdout.text;
        let stderr = output.stderr.text;
        let exit_code = output.status.code().unwrap_or(-1);
        let structured_cdp_args = if tool_name == BuiltinTool::ChatPlusAdapterDebugger.name()
            && cdp_args.first().map(|arg| arg.as_str()) == Some("eval")
        {
            vec!["eval".to_string(), "[eval omitted]".to_string()]
        } else {
            cdp_args.clone()
        };

        let structured = json!({
            "status": if output.status.success() { "completed" } else { "failed" },
            "tool": tool_name,
            "command": structured_command,
            "runner": node_command(),
            "script": normalize_display_path(&cdp_script),
            "args": structured_cdp_args,
            "runtimeDir": normalize_display_path(&cdp_runtime_dir),
            "userDataDir": normalize_display_path(&effective_user_data_dir),
            "profileMode": effective_profile_mode,
            "browserMode": effective_browser_mode,
            "exitCode": exit_code,
            "durationMs": duration_ms,
            "stdoutTruncated": output.stdout.truncated,
            "stderrTruncated": output.stderr.truncated
        });
        let output_text = command_output_text(&stdout, &stderr);

        if output.status.success() {
            Ok(tool_success_with_planning_reminder(
                output_text,
                structured,
                self.planning_success_hints(
                    config,
                    planning_scope,
                    planning_id,
                    planning_tool,
                    None,
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

    async fn handle_builtin_chat_plus_adapter_debugger(
        &self,
        config: &GatewayConfig,
        args: BuiltinShellArgs,
        planning_scope: &str,
    ) -> Result<ToolResult, AppError> {
        let command_preview = args.exec.trim().to_string();
        if command_preview.is_empty() {
            return Err(AppError::BadRequest("exec cannot be empty".to_string()));
        }

        if let Some((doc_tool, matched_path)) = builtin_skill_doc_read(&command_preview) {
            return Ok(builtin_skill_doc_result(
                doc_tool,
                &command_preview,
                matched_path,
                builtin_skill_token(doc_tool),
                Self::planning_enabled(config),
            ));
        }

        if let Some(result) = validate_skill_token_result(
            BuiltinTool::ChatPlusAdapterDebugger.name(),
            &builtin_skill_token(BuiltinTool::ChatPlusAdapterDebugger),
            args.skill_token.as_deref(),
        ) {
            return Ok(result);
        }

        let Some(debug_command) = parse_chat_plus_debug_command(&command_preview)? else {
            return Ok(tool_error(
                format!(
                    "{} supports documentation reads and Chrome CDP debugging actions. Use `capture start`, `network search <filter>`, `network get <request-id>`, or a documented CDP command after reading {}.",
                    BuiltinTool::ChatPlusAdapterDebugger.name(),
                    builtin_skill_uri(BuiltinTool::ChatPlusAdapterDebugger)
                ),
                json!({
                    "status": "error",
                    "tool": BuiltinTool::ChatPlusAdapterDebugger.name(),
                    "exec": command_preview,
                    "nextStep": "Use one of: capture start, capture clear, network search <filter>, network get <request-id>, network perf, netclear, net, netget, perfnet, html, snap, evalraw"
                }),
            ));
        };

        if let Some(result) = self
            .check_planning_gate(
                config,
                planning_scope,
                BuiltinTool::ChatPlusAdapterDebugger,
                args.planning_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        match debug_command {
            ChatPlusDebugCommand::Cdp {
                command,
                structured_command,
            } => {
                self.execute_builtin_chrome_cdp_command(
                    config,
                    BuiltinTool::ChatPlusAdapterDebugger.name(),
                    &command,
                    &structured_command,
                    args.timeout_ms,
                    planning_scope,
                    args.planning_id.as_deref(),
                    BuiltinTool::ChatPlusAdapterDebugger,
                )
                .await
            }
            ChatPlusDebugCommand::CaptureStart | ChatPlusDebugCommand::CaptureClear => {
                self.execute_builtin_chrome_cdp_command(
                    config,
                    BuiltinTool::ChatPlusAdapterDebugger.name(),
                    "netclear",
                    &command_preview,
                    args.timeout_ms,
                    planning_scope,
                    args.planning_id.as_deref(),
                    BuiltinTool::ChatPlusAdapterDebugger,
                )
                .await
            }
        }
    }

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

fn tool_error(text: String, structured: Value) -> ToolResult {
    ToolResult {
        text,
        structured,
        is_error: true,
    }
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

fn builtin_tools(cfg: &BuiltinToolsConfig) -> Vec<BuiltinTool> {
    let mut tools = Vec::with_capacity(6);
    if cfg.shell_command {
        tools.push(BuiltinTool::ShellCommand);
    }
    if cfg.apply_patch {
        tools.push(BuiltinTool::ApplyPatch);
    }
    if cfg.multi_edit_file {
        tools.push(BuiltinTool::MultiEditFile);
    }
    if cfg.task_planning {
        tools.push(BuiltinTool::TaskPlanning);
    }
    if cfg.chrome_cdp {
        tools.push(BuiltinTool::ChromeCdp);
    }
    if cfg.chat_plus_adapter_debugger {
        tools.push(BuiltinTool::ChatPlusAdapterDebugger);
    }
    tools
}

fn external_skill_tool_definitions(skills: &[DiscoveredSkill]) -> Value {
    let bindings = build_skill_tool_bindings(skills);
    let now = Utc::now().to_rfc3339();
    let os = current_os_label();
    let tools: Vec<Value> = bindings
        .into_iter()
        .map(|(tool_name, skill)| {
            let description = render_skill_tool_description(skill, os, &now);
            json!({
                "name": tool_name,
                "description": description,
                "inputSchema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["exec"],
                    "properties": {
                        "exec": {
                            "type": "string",
                            "description": "Shell command string for this skill."
                        },
                        "skillToken": {
                            "type": "string",
                            "description": "Required skill token."
                        },

                    }
                }
            })
        })
        .collect();
    Value::Array(tools)
}

fn builtin_tool_definitions(os: &str, now: &str, cfg: &BuiltinToolsConfig) -> Vec<Value> {
    let enabled: Vec<BuiltinTool> = builtin_tools(cfg);
    let mut defs = Vec::with_capacity(6);

    if enabled.contains(&BuiltinTool::ShellCommand) {
        defs.push(json!({
            "name": BuiltinTool::ShellCommand.name(),
            "description": render_builtin_tool_description(BuiltinTool::ShellCommand, os, now, cfg.task_planning),
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
                    }
                }
            }
        }));
    }
    if enabled.contains(&BuiltinTool::ApplyPatch) {
        defs.push(json!({
            "name": BuiltinTool::ApplyPatch.name(),
            "description": render_builtin_tool_description(BuiltinTool::ApplyPatch, os, now, cfg.task_planning),
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
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for every apply_patch call. First read the complete builtin://apply_patch/SKILL.md with shell_command without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Calls without the correct token fail and must be retried."
                    }
                }
            }
        }));
    }
    if enabled.contains(&BuiltinTool::MultiEditFile) {
        defs.push(json!({
            "name": BuiltinTool::MultiEditFile.name(),
            "description": render_builtin_tool_description(BuiltinTool::MultiEditFile, os, now, cfg.task_planning),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path", "edits"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the existing file to modify, relative to cwd unless absolute."
                    },
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "description": "Ordered exact string replacements. All edits are validated and applied in memory before the file is written once.",
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
                    "cwd": {
                        "type": "string",
                        "description": "Concrete working directory for relative paths. It must be inside one configured allowed directory. Required when more than one allowed directory exists; do not omit it in that case."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for every multi_edit_file call. First read the complete builtin://multi_edit_file/SKILL.md with shell_command without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Calls without the correct token fail and must be retried."
                    }
                }
            }
        }));
    }
    if enabled.contains(&BuiltinTool::TaskPlanning) {
        defs.push(json!({
            "name": BuiltinTool::TaskPlanning.name(),
            "description": render_builtin_tool_description(BuiltinTool::TaskPlanning, os, now, cfg.task_planning),
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
        }));
    }
    if enabled.contains(&BuiltinTool::ChromeCdp) {
        defs.push(json!({
            "name": BuiltinTool::ChromeCdp.name(),
            "description": render_builtin_tool_description(BuiltinTool::ChromeCdp, os, now, cfg.task_planning),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["exec"],
                "properties": {
                    "exec": {
                        "type": "string",
                        "description": "Chrome DevTools Protocol command. First call must read the complete builtin://chrome-cdp/SKILL.md. After reading it, use commands like `open <url>`, `list`, `snap`, `eval`, `netclear`, `net <filter>`, `netget <id>`, or `click <target> <selector>`."
                    },
                    "timeoutMs": {
                        "type": "integer",
                        "minimum": 1000,
                        "description": "Optional command timeout in milliseconds."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for every non-documentation call. First read the complete builtin://chrome-cdp/SKILL.md without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Calls without the correct token fail and must be retried."
                    }
                }
            }
        }));
    }
    if enabled.contains(&BuiltinTool::ChatPlusAdapterDebugger) {
        defs.push(json!({
            "name": BuiltinTool::ChatPlusAdapterDebugger.name(),
            "description": render_builtin_tool_description(BuiltinTool::ChatPlusAdapterDebugger, os, now, cfg.task_planning),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["exec"],
                "properties": {
                    "exec": {
                        "type": "string",
                        "description": "Documentation read or Chrome CDP debugging action. First call must read the complete builtin://chat-plus-adapter-debugger/SKILL.md. Then use `capture start`, `network search <filter>`, `network get <request-id>`, `network perf`, or documented raw CDP commands such as `netclear`, `net`, `netget`, `html`, `snap`, and `evalraw`."
                    },
                    "timeoutMs": {
                        "type": "integer",
                        "minimum": 1000,
                        "description": "Optional CDP command timeout in milliseconds."
                    },
                    "skillToken": {
                        "type": "string",
                        "description": "Required for every non-documentation action. First read the complete builtin://chat-plus-adapter-debugger/SKILL.md without skillToken, then use the returned skillToken; do not use regex or partial reads to fetch only the token. Calls without the correct token fail and must be retried."
                    }
                }
            }
        }));
    }
    if cfg.task_planning {
        for def in &mut defs {
            add_planning_gate_schema(def);
        }
    }
    defs
}

fn add_planning_gate_schema(def: &mut Value) {
    let Some(name) = def.get("name").and_then(Value::as_str) else {
        return;
    };
    if name == BuiltinTool::TaskPlanning.name() {
        return;
    }
    let Some(properties) = def
        .get_mut("inputSchema")
        .and_then(|schema| schema.get_mut("properties"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    properties.insert(
        "planningId".to_string(),
        json!({
            "type": "string",
            "description": "Required for non-documentation calls when task-planning is enabled. Use the planningId returned by task-planning update."
        }),
    );
}

fn render_builtin_tool_description(
    tool: BuiltinTool,
    os: &str,
    now: &str,
    planning_enabled: bool,
) -> String {
    let frontmatter = builtin_skill_frontmatter(tool);
    let skill_uri = builtin_skill_uri(tool);
    let skill_root_uri = builtin_skill_uri_root(tool);
    let read_cmd = if cfg!(target_os = "windows") {
        format!("Get-Content -Raw {skill_uri}")
    } else {
        format!("cat {skill_uri}")
    };
    let read_requirement = match tool {
        BuiltinTool::ShellCommand => {
            format!("The only acceptable first call to this tool is a documentation-read call that reads the complete SKILL.md and does not require `skillToken`. Suggested `exec`: `{read_cmd}`.")
        }
        BuiltinTool::ApplyPatch | BuiltinTool::MultiEditFile => {
            format!("Before calling `{}`, use `shell_command` to read the complete SKILL.md; this read does not require `skillToken`. Suggested shell `exec`: `{read_cmd}`.", tool.name())
        }
        BuiltinTool::TaskPlanning
        | BuiltinTool::ChromeCdp
        | BuiltinTool::ChatPlusAdapterDebugger => {
            format!("The only acceptable first call to this tool is a documentation-read call that reads the complete SKILL.md and does not require `skillToken`, using `exec`: `{read_cmd}`.")
        }
    };
    let frontmatter_block = if frontmatter.block.trim().is_empty() {
        "none".to_string()
    } else {
        format!("---\n{}\n---", frontmatter.block.trim())
    };

    let mut description = format!(
        "Bundled skill: {}.\nMANDATORY BEFORE USE: this tool description is only a short discovery summary, not the operating instructions. Before using this bundled skill for any real action, you MUST first read its full SKILL.md. Do not infer safe usage from this description alone; skipping SKILL.md can cause incorrect or dangerous tool use. {read_requirement} The SKILL.md response includes the required `skillToken` only inside the returned markdown content. You must obtain it by reading the complete SKILL.md document; this SKILL.md read is the one call that does not need `skillToken`. Do not use regex, grep, Select-String, line ranges, or other partial-read tricks to fetch only the token. Every later non-documentation call to this skill MUST include that exact `skillToken` argument or the gateway will reject the call; a rejected call fails and must be retried with the correct token. The gateway serves bundled SKILL.md reads from embedded content, so this direct documentation read does not require a workspace `cwd`.\nCurrent OS: {os}.\nCurrent datetime: {now}.\nSkill URI: {skill_root_uri}.\nSKILL.md URI: {skill_uri}.\nFront matter summary:\nname: {}\ndescription: {}\nmetadata: {}\nFront matter raw (YAML):\n{}",
        frontmatter.name,
        frontmatter.name,
        if frontmatter.description.trim().is_empty() {
            "none"
        } else {
            frontmatter.description.trim()
        },
        if frontmatter.metadata.trim().is_empty() {
            "none"
        } else {
            frontmatter.metadata.trim()
        },
        frontmatter_block
    );
    if planning_enabled && tool != BuiltinTool::TaskPlanning {
        description.push_str("\n\n");
        description.push_str(planning_gate_instructions());
    }
    description
}

fn builtin_skill_frontmatter(tool: BuiltinTool) -> ParsedFrontmatter {
    parse_frontmatter_content(builtin_skill_md_content(tool), &builtin_skill_uri(tool))
        .unwrap_or_default()
}

fn builtin_skill_md_content(tool: BuiltinTool) -> &'static str {
    match tool {
        BuiltinTool::ShellCommand => BUILTIN_SHELL_COMMAND_SKILL_MD,
        BuiltinTool::ApplyPatch => BUILTIN_APPLY_PATCH_SKILL_MD,
        BuiltinTool::MultiEditFile => BUILTIN_MULTI_EDIT_FILE_SKILL_MD,
        BuiltinTool::TaskPlanning => BUILTIN_TASK_PLANNING_SKILL_MD,
        BuiltinTool::ChromeCdp => BUILTIN_CHROME_CDP_SKILL_MD,
        BuiltinTool::ChatPlusAdapterDebugger => BUILTIN_CHAT_PLUS_ADAPTER_DEBUGGER_SKILL_MD,
    }
}

fn builtin_skills_root_uri() -> &'static str {
    "builtin://"
}

fn builtin_skill_uri_root(tool: BuiltinTool) -> String {
    format!("builtin://{}", tool.name())
}

fn builtin_skill_uri(tool: BuiltinTool) -> String {
    format!("builtin://{}/SKILL.md", tool.name())
}

fn builtin_skill_doc_read(command: &str) -> Option<(BuiltinTool, String)> {
    let tokens = split_shell_tokens(command);
    let (program, args) = tokens.split_first()?;
    let normalized_program = normalize_command_token(program);
    if !matches!(
        normalized_program.as_str(),
        "cat" | "type" | "get-content" | "gc"
    ) {
        return None;
    }

    args.iter().find_map(|arg| builtin_skill_doc_arg(arg))
}

fn builtin_skill_doc_result(
    tool: BuiltinTool,
    command: &str,
    matched_path: String,
    token: String,
    planning_enabled: bool,
) -> ToolResult {
    let mut text = render_builtin_skill_md(tool, planning_enabled);
    text.push_str(&format!(
        "\n\n[skillToken]\nUse this exact skillToken for subsequent non-documentation calls to `{}`: {}\n",
        tool.name(),
        token
    ));
    tool_success(
        text,
        json!({
            "status": "completed",
            "tool": BuiltinTool::ShellCommand.name(),
            "command": command,
            "builtinSkill": tool.name(),
            "path": matched_path,
            "source": "embedded",
            "runtimeAssets": json!({"status": "none"})
        }),
    )
}

fn render_builtin_skill_md(tool: BuiltinTool, planning_enabled: bool) -> String {
    let mut content = builtin_skill_md_content(tool).to_string();
    if planning_enabled && tool != BuiltinTool::TaskPlanning {
        content.push_str("\n\n");
        content.push_str(planning_gate_instructions());
    }
    content
}

fn materialize_builtin_chrome_cdp_script() -> Result<PathBuf, AppError> {
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("mcp-gateway")
        .join("builtin-skills")
        .join("chrome-cdp")
        .join("scripts");
    fs::create_dir_all(&dir)?;
    let path = dir.join("cdp.mjs");
    let should_write = match fs::read_to_string(&path) {
        Ok(existing) => existing != BUILTIN_CHROME_CDP_MJS,
        Err(_) => true,
    };
    if should_write {
        fs::write(&path, BUILTIN_CHROME_CDP_MJS)?;
    }
    Ok(path)
}

fn skill_doc_result(
    tool_name: &str,
    skill: &str,
    command: &str,
    path: String,
    content: String,
    token: String,
) -> ToolResult {
    let mut text = content;
    text.push_str(&format!(
        "\n\n[skillToken]\nUse this exact skillToken for subsequent non-documentation calls to `{tool_name}`: {token}\n"
    ));
    tool_success(
        text,
        json!({
            "status": "completed",
            "tool": tool_name,
            "skill": skill,
            "command": command,
            "path": path,
            "source": "file"
        }),
    )
}

fn validate_skill_token_result(
    tool_name: &str,
    expected_token: &str,
    provided: Option<&str>,
) -> Option<ToolResult> {
    match provided.map(str::trim).filter(|value| !value.is_empty()) {
        Some(provided) if provided == expected_token => None,
        Some(_) => Some(skill_token_error(
            tool_name,
            "invalid skillToken for this skill",
        )),
        None => Some(skill_token_error(
            tool_name,
            "missing skillToken for this skill",
        )),
    }
}

fn skill_token_error(tool_name: &str, message: &str) -> ToolResult {
    tool_error(
        format!(
            "{message}. This call failed and must be retried with the correct token. Read the complete SKILL.md first; that documentation-read call does not require `skillToken`. Then retry `{tool_name}` with the returned `skillToken` argument. Do not use regex, grep, Select-String, line ranges, or partial reads to fetch only the token."
        ),
        json!({
            "status": "error",
            "code": "SkillTokenRequired",
            "tool": tool_name,
            "message": message,
            "requiredArgument": "skillToken",
            "nextStep": "This call failed. Read the complete corresponding SKILL.md with the documented first-call command; that SKILL.md read does not require skillToken. Then retry with the returned skillToken. Do not use regex, grep, Select-String, line ranges, or partial reads to fetch only the token."
        }),
    )
}

fn builtin_skill_token(tool: BuiltinTool) -> String {
    skill_token_from_content(builtin_skill_md_content(tool))
}

fn external_skill_token(skill: &DiscoveredSkill) -> Result<String, AppError> {
    let skill_md_path = skill.path.join("SKILL.md");
    let content = std::fs::read_to_string(skill_md_path)?;
    Ok(skill_token_from_content(&content))
}

fn skill_token_from_content(content: &str) -> String {
    // Stable FNV-1a hash: enough for a short gate token without adding a crypto dependency.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}").chars().take(6).collect()
}

fn is_external_skill_doc_read_command(command: &str, skill: &DiscoveredSkill) -> bool {
    let tokens = split_shell_tokens(command);
    let Some((program, args)) = tokens.split_first() else {
        return false;
    };
    let normalized_program = normalize_command_token(program);
    if !matches!(
        normalized_program.as_str(),
        "cat" | "type" | "get-content" | "gc"
    ) {
        return false;
    }

    let skill_md = normalize_root_path(skill.path.join("SKILL.md"));
    args.iter().any(|arg| {
        let candidate = strip_matching_quotes(arg)
            .trim()
            .trim_end_matches(';')
            .trim();
        if candidate.is_empty() || candidate.starts_with('-') {
            return false;
        }
        let path = PathBuf::from(candidate);
        let resolved = if path.is_absolute() {
            normalize_root_path(path)
        } else {
            normalize_root_path(skill.path.join(path))
        };
        resolved == skill_md
    })
}

fn builtin_skill_doc_arg(arg: &str) -> Option<(BuiltinTool, String)> {
    let candidate = strip_matching_quotes(arg)
        .trim()
        .trim_end_matches(';')
        .trim();
    if candidate.is_empty() || candidate.starts_with('-') {
        return None;
    }

    for tool in builtin_tools(&BuiltinToolsConfig::default()) {
        let uri = builtin_skill_uri(tool);
        if candidate.eq_ignore_ascii_case(&uri) {
            return Some((tool, uri));
        }
    }

    None
}

fn parse_builtin_chrome_cdp_args(command: &str) -> Result<Vec<String>, AppError> {
    let tokens = split_shell_tokens(command);
    let Some((program, args)) = tokens.split_first() else {
        return Err(AppError::BadRequest("exec cannot be empty".to_string()));
    };
    let normalized_program = normalize_command_token(program);

    if is_chrome_cdp_node_invocation(&normalized_program, args) {
        return Ok(args[1..].to_vec());
    }

    if is_chrome_cdp_script_token(program) || normalized_program == "cdp" {
        return Ok(args.to_vec());
    }

    if is_chrome_cdp_cli_command(&normalized_program) || is_chrome_cdp_cli_flag(&normalized_program)
    {
        return Ok(tokens);
    }

    Err(AppError::BadRequest(
        "chrome-cdp uses the bundled raw CDP runner. Command must be a documented CDP subcommand such as `open`, `list`, `snap`, `eval`, `netclear`, `net`, `netget`, `click`, or `stop` after SKILL.md has been read".to_string(),
    ))
}

#[derive(Debug)]
enum ChatPlusDebugCommand {
    Cdp {
        command: String,
        structured_command: String,
    },
    CaptureStart,
    CaptureClear,
}

fn parse_chat_plus_debug_command(command: &str) -> Result<Option<ChatPlusDebugCommand>, AppError> {
    let tokens = split_shell_tokens(command);
    let Some(first) = tokens.first() else {
        return Ok(None);
    };
    let first = first.to_ascii_lowercase();

    match first.as_str() {
        "capture" => parse_chat_plus_capture_command(&tokens),
        "network" => parse_chat_plus_network_command(&tokens),
        "launch" | "open" | "list" | "ls" | "netclear" | "network-clear" | "net" | "netget"
        | "network-get" | "perfnet" | "html" | "snap" | "snapshot" | "evalraw" | "eval"
        | "shot" | "screenshot" | "nav" | "navigate" | "click" | "clickxy" | "type" | "loadall"
        | "stop" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts(&tokens)?,
            structured_command: command.to_string(),
        })),
        _ => Ok(None),
    }
}

fn parse_chat_plus_capture_command(
    tokens: &[String],
) -> Result<Option<ChatPlusDebugCommand>, AppError> {
    let Some(action) = tokens.get(1).map(|value| value.to_ascii_lowercase()) else {
        return Ok(None);
    };
    match action.as_str() {
        "start" | "install" => Ok(Some(ChatPlusDebugCommand::CaptureStart)),
        "clear" | "reset" => Ok(Some(ChatPlusDebugCommand::CaptureClear)),
        "list" | "search" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("net", &tokens[2..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
        "get" => {
            if tokens.len() < 3 {
                return Err(AppError::BadRequest(
                    "capture get requires a CDP request id".to_string(),
                ));
            }
            Ok(Some(ChatPlusDebugCommand::Cdp {
                command: cdp_command_from_parts_with_prefix("netget", &tokens[2..])?,
                structured_command: cdp_command_from_parts(tokens)?,
            }))
        }
        "perf" | "performance" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("perfnet", &tokens[2..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
        _ => Ok(None),
    }
}

fn parse_chat_plus_network_command(
    tokens: &[String],
) -> Result<Option<ChatPlusDebugCommand>, AppError> {
    let Some(action) = tokens.get(1).map(|value| value.to_ascii_lowercase()) else {
        return Ok(Some(ChatPlusDebugCommand::Cdp {
            command: "net".to_string(),
            structured_command: "network".to_string(),
        }));
    };
    match action.as_str() {
        "clear" | "start" | "reset" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("netclear", &tokens[2..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
        "list" | "search" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("net", &tokens[2..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
        "get" => {
            if tokens.len() < 3 {
                return Err(AppError::BadRequest(
                    "network get requires a CDP request id".to_string(),
                ));
            }
            Ok(Some(ChatPlusDebugCommand::Cdp {
                command: cdp_command_from_parts_with_prefix("netget", &tokens[2..])?,
                structured_command: cdp_command_from_parts(tokens)?,
            }))
        }
        "perf" | "performance" => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("perfnet", &tokens[2..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
        _ => Ok(Some(ChatPlusDebugCommand::Cdp {
            command: cdp_command_from_parts_with_prefix("net", &tokens[1..])?,
            structured_command: cdp_command_from_parts(tokens)?,
        })),
    }
}

fn cdp_command_from_parts_with_prefix(prefix: &str, parts: &[String]) -> Result<String, AppError> {
    let mut all = Vec::with_capacity(parts.len() + 1);
    all.push(prefix.to_string());
    all.extend(parts.iter().cloned());
    cdp_command_from_parts(&all)
}

fn cdp_command_from_parts(parts: &[String]) -> Result<String, AppError> {
    parts
        .iter()
        .map(|part| quote_cdp_command_part(part))
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join(" "))
}

fn quote_cdp_command_part(part: &str) -> Result<String, AppError> {
    if part.is_empty() {
        return Ok("\"\"".to_string());
    }
    if !part
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '\'' || ch == '"')
    {
        return Ok(part.to_string());
    }
    if !part.contains('"') {
        return Ok(format!("\"{part}\""));
    }
    if !part.contains('\'') {
        return Ok(format!("'{part}'"));
    }
    Err(AppError::BadRequest(
        "CDP command arguments cannot contain both single and double quotes".to_string(),
    ))
}

fn is_chrome_cdp_node_invocation(normalized_program: &str, args: &[String]) -> bool {
    matches!(normalized_program, "node" | "node.exe")
        && args
            .first()
            .is_some_and(|arg| is_chrome_cdp_script_token(arg))
}

fn is_chrome_cdp_script_token(token: &str) -> bool {
    normalize_command_token(token).ends_with("cdp.mjs")
}

fn is_chrome_cdp_cli_flag(command: &str) -> bool {
    matches!(command, "--help" | "-v" | "-V" | "--version" | "--full")
}

fn is_chrome_cdp_cli_command(command: &str) -> bool {
    matches!(
        command,
        "launch"
            | "open"
            | "list"
            | "ls"
            | "snap"
            | "snapshot"
            | "screenshot"
            | "shot"
            | "click"
            | "clickxy"
            | "type"
            | "eval"
            | "html"
            | "nav"
            | "navigate"
            | "net"
            | "network"
            | "netclear"
            | "network-clear"
            | "netget"
            | "network-get"
            | "perfnet"
            | "loadall"
            | "evalraw"
            | "stop"
            | "help"
    )
}

fn builtin_chrome_cdp_runtime_dir() -> Result<PathBuf, AppError> {
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("mcp-gateway")
        .join("builtin-skills")
        .join("chrome-cdp")
        .join("runtime");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn builtin_chrome_cdp_user_data_dir() -> Result<PathBuf, AppError> {
    let dir = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("mcp-gateway")
        .join("builtin-skills")
        .join("chrome-cdp")
        .join("chrome-user-data");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(target_os = "windows")]
fn node_command() -> &'static str {
    "node.exe"
}

#[cfg(not(target_os = "windows"))]
fn node_command() -> &'static str {
    "node"
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
            value if value.eq_ignore_ascii_case(Self::MultiEditFile.name()) => {
                Some(Self::MultiEditFile)
            }
            value if value.eq_ignore_ascii_case(Self::TaskPlanning.name()) => {
                Some(Self::TaskPlanning)
            }
            value if value.eq_ignore_ascii_case(Self::ChromeCdp.name()) => Some(Self::ChromeCdp),
            value if value.eq_ignore_ascii_case(Self::ChatPlusAdapterDebugger.name()) => {
                Some(Self::ChatPlusAdapterDebugger)
            }
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::ShellCommand => "shell_command",
            Self::ApplyPatch => "apply_patch",
            Self::MultiEditFile => "multi_edit_file",
            Self::TaskPlanning => "task-planning",
            Self::ChromeCdp => "chrome-cdp",
            Self::ChatPlusAdapterDebugger => "chat-plus-adapter-debugger",
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
        "MANDATORY BEFORE USE: this tool description is only a short discovery summary, not the operating instructions. Before using this skill for any real action, you MUST first call this skill tool with `exec` that reads the full SKILL.md from the skill path below. Do not infer safe usage from this description alone; skipping SKILL.md can cause incorrect or dangerous tool use. The only acceptable first call is a documentation-read call that reads the complete SKILL.md without `skillToken`, such as `cat {skill_path}/SKILL.md` or `Get-Content -Raw {skill_path}/SKILL.md`. The SKILL.md response includes the required `skillToken` only inside the returned markdown content. You must obtain it by reading the complete SKILL.md document; this SKILL.md read is the one call that does not need `skillToken`. Do not use regex, grep, Select-String, line ranges, or other partial-read tricks to fetch only the token. Every later non-documentation call to this skill MUST include that exact `skillToken` argument or the gateway will reject the call; a rejected call fails and must be retried with the correct token.\nThe `exec` value should be one shell command string used either to read markdown files or run scripts after SKILL.md has been read.\nCurrent OS: {os}.\nCurrent datetime: {now}.\nSkill path: {skill_path}.\nFront matter summary:\nname: {}\ndescription: {}\nmetadata: {}\nFront matter raw (YAML):\n{}",
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
    parse_frontmatter_content(&content, &skill_md_path.display().to_string())
}

fn parse_frontmatter_content(content: &str, source: &str) -> Result<ParsedFrontmatter, AppError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
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
        AppError::BadRequest(format!("invalid YAML frontmatter in {source}: {error}"))
    })?;
    let frontmatter_obj = frontmatter.as_object().ok_or_else(|| {
        AppError::BadRequest(format!("frontmatter in {source} must be a YAML mapping"))
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

fn shell_command_for_current_os(exec: &str) -> (String, Vec<String>) {
    if cfg!(target_os = "windows") {
        let runner = "powershell".to_string();
        let args = vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-Command".to_string(),
            exec.to_string(),
        ];
        wrap_windows_powershell_command_for_utf8(&runner, &args).unwrap_or((runner, args))
    } else {
        ("sh".to_string(), vec!["-lc".to_string(), exec.to_string()])
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
    stdout_emitter: Option<SkillStreamEmitter>,
    stderr_emitter: Option<SkillStreamEmitter>,
) -> Result<SkillCommandExecution, AppError> {
    let mut child = command
        .spawn()
        .map_err(|error| AppError::Upstream(format!("failed to execute command: {error}")))?;
    if let Some(pid) = child.id() {
        let _ = assign_child_to_gateway_job(pid);
    }
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
        stdout_emitter,
    ));
    let stderr_task = tokio::spawn(capture_stream_output(
        stderr,
        stderr_state.clone(),
        max_output_bytes,
        disable_truncation,
        stderr_emitter,
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
    emitter: Option<SkillStreamEmitter>,
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
        if let Some(emitter) = &emitter {
            emitter
                .emit(String::from_utf8_lossy(&chunk[..read]).to_string())
                .await;
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

fn resolve_builtin_cwd(
    tool: BuiltinTool,
    skills: &SkillsConfig,
    cwd: Option<&str>,
) -> Result<PathBuf, ToolResult> {
    let allowed_roots = configured_allowed_dir_paths(skills);
    let allowed_dirs = allowed_roots
        .iter()
        .map(|path| normalize_display_path(path))
        .collect::<Vec<_>>();
    if allowed_roots.is_empty() {
        return Err(cwd_error_result(
            tool,
            "skills requires at least one allowed directory",
            cwd,
            None,
            allowed_dirs,
        ));
    }

    let selected = if let Some(cwd) = cwd.map(str::trim).filter(|value| !value.is_empty()) {
        PathBuf::from(cwd)
    } else {
        match allowed_roots.as_slice() {
            [only] => only.clone(),
            _ => {
                let message = "cwd is required because multiple allowed directories are configured; ask the user which directory to operate in";
                return Err(cwd_error_result(tool, message, cwd, None, allowed_dirs));
            }
        }
    };
    let normalized = normalize_root_path(selected);
    if !allowed_roots
        .iter()
        .any(|root| normalized.starts_with(root))
    {
        let message = "cwd must be inside one configured allowed directory";
        return Err(cwd_error_result(
            tool,
            message,
            cwd,
            Some(&normalized),
            allowed_dirs,
        ));
    }

    if !normalized.exists() || !normalized.is_dir() {
        let message = format!(
            "working directory must be an existing directory: {}",
            normalized.to_string_lossy()
        );
        return Err(cwd_error_result(
            tool,
            &message,
            cwd,
            Some(&normalized),
            allowed_dirs,
        ));
    }

    Ok(normalized)
}

fn cwd_error_result(
    tool: BuiltinTool,
    message: &str,
    cwd: Option<&str>,
    resolved_cwd: Option<&Path>,
    allowed_dirs: Vec<String>,
) -> ToolResult {
    let requested_cwd = cwd
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let resolved_cwd = resolved_cwd.map(normalize_display_path);
    tool_error(
        format!(
            "{message}\nAllowed directories:\n{}",
            allowed_dirs_text(&allowed_dirs)
        ),
        json!({
            "status": "error",
            "code": "InvalidCwd",
            "message": message,
            "tool": tool.name(),
            "requestedCwd": requested_cwd,
            "resolvedCwd": resolved_cwd,
            "allowedDirectories": allowed_dirs,
            "nextStep": "Ask the user which allowed directory should be used as cwd, then retry with cwd set to that directory or a child directory."
        }),
    )
}

fn allowed_dirs_text(allowed_dirs: &[String]) -> String {
    if allowed_dirs.is_empty() {
        return "- <none configured>".to_string();
    }
    allowed_dirs
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn configured_allowed_dir_paths(skills: &SkillsConfig) -> Vec<PathBuf> {
    skills
        .policy
        .path_guard
        .whitelist_dirs
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| normalize_root_path(PathBuf::from(value)))
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
    let normalized_input = normalize_apply_patch_input(input);
    let lines = normalized_input.lines().collect::<Vec<_>>();
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
        let header = line.trim();
        if let Some(path) = header.strip_prefix("*** Add File: ") {
            index += 1;
            let mut contents = Vec::new();
            while index + 1 < lines.len() && !is_patch_file_header(lines[index]) {
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
        if let Some(path) = header.strip_prefix("*** Delete File: ") {
            hunks.push(PatchHunk::DeleteFile {
                path: path.trim().to_string(),
            });
            index += 1;
            continue;
        }
        if let Some(path) = header.strip_prefix("*** Update File: ") {
            index += 1;
            let mut move_path = None;
            if index + 1 < lines.len() {
                if let Some(target) = lines[index].trim().strip_prefix("*** Move to: ") {
                    move_path = Some(target.trim().to_string());
                    index += 1;
                }
            }
            let mut chunks = Vec::new();
            let mut current = PatchChunk::default();
            let mut in_chunk = false;
            while index + 1 < lines.len() && !is_patch_file_header(lines[index]) {
                let patch_line = lines[index];
                if patch_line == "@@" || patch_line.starts_with("@@ ") {
                    push_patch_chunk(&mut chunks, &mut current);
                    in_chunk = true;
                    current.change_context = patch_line
                        .strip_prefix("@@ ")
                        .map(|context| context.to_string());
                    index += 1;
                    continue;
                }
                if patch_line == "*** End of File" {
                    in_chunk = true;
                    current.is_end_of_file = true;
                    index += 1;
                    continue;
                }
                let Some(prefix) = patch_line.chars().next() else {
                    if in_chunk {
                        current.old_lines.push(String::new());
                        current.new_lines.push(String::new());
                    }
                    index += 1;
                    continue;
                };
                let body = patch_line.get(1..).unwrap_or_default().to_string();
                match prefix {
                    ' ' => {
                        in_chunk = true;
                        current.old_lines.push(body.clone());
                        current.new_lines.push(body);
                    }
                    '-' => {
                        in_chunk = true;
                        current.old_lines.push(body);
                    }
                    '+' => {
                        in_chunk = true;
                        current.new_lines.push(body);
                    }
                    _ => {
                        return Err(AppError::BadRequest(format!(
                            "invalid update hunk line: {patch_line}"
                        )));
                    }
                }
                index += 1;
            }
            push_patch_chunk(&mut chunks, &mut current);
            if chunks.is_empty()
                || chunks
                    .iter()
                    .any(|chunk| chunk.old_lines.is_empty() && chunk.new_lines.is_empty())
            {
                return Err(AppError::BadRequest(format!(
                    "update file hunk for path '{}' is empty",
                    path.trim()
                )));
            }
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

fn normalize_apply_patch_input(input: &str) -> String {
    let trimmed = input.trim();
    let lines = trimmed.lines().collect::<Vec<_>>();
    if lines.len() >= 4 {
        let first = lines.first().map(|line| line.trim());
        let last = lines.last().map(|line| line.trim());
        if matches!(first, Some("<<EOF" | "<<'EOF'" | "<<\"EOF\"")) && last == Some("EOF") {
            return lines[1..lines.len() - 1].join("\n").trim().to_string();
        }
    }
    trimmed.to_string()
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
    if current.old_lines.is_empty()
        && current.new_lines.is_empty()
        && current.change_context.is_none()
        && !current.is_end_of_file
    {
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

fn patch_preview_changes(parsed: &ParsedPatch) -> Value {
    let changes = parsed
        .hunks
        .iter()
        .map(|hunk| match hunk {
            PatchHunk::AddFile { path, contents } => json!({
                "kind": "add",
                "path": path,
                "lines": contents.len()
            }),
            PatchHunk::DeleteFile { path } => json!({
                "kind": "delete",
                "path": path
            }),
            PatchHunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => json!({
                "kind": "update",
                "path": path,
                "movePath": move_path,
                "chunks": chunks.iter().map(|chunk| json!({
                    "context": chunk.change_context.as_deref(),
                    "oldLines": chunk.old_lines.len(),
                    "newLines": chunk.new_lines.len(),
                    "endOfFile": chunk.is_end_of_file
                })).collect::<Vec<_>>()
            }),
        })
        .collect::<Vec<_>>();
    json!(changes)
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

fn apply_parsed_patch(
    parsed: &ParsedPatch,
    cwd: &Path,
) -> Result<ApplyPatchOutcome, ApplyPatchFailure> {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    let mut delta = AppliedPatchDelta::default();
    let mut warnings = Vec::new();

    for hunk in &parsed.hunks {
        match hunk {
            PatchHunk::AddFile { path, contents } => {
                let target =
                    resolve_patch_path(cwd, path).map_err(|error| patch_failure(error, &delta))?;
                if target.is_dir() {
                    return Err(patch_failure(
                        AppError::BadRequest(format!(
                            "add file target is a directory: {}",
                            target.to_string_lossy()
                        )),
                        &delta,
                    ));
                }
                let overwritten_content = match fs::read_to_string(&target) {
                    Ok(content) => Some(content),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(_) => {
                        delta.exact = false;
                        None
                    }
                };
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| patch_failure(error.into(), &delta))?;
                }
                let content = format!("{}\n", contents.join("\n"));
                if let Err(error) = fs::write(&target, &content) {
                    delta.exact = false;
                    return Err(patch_failure(error.into(), &delta));
                }
                warnings.extend(collect_edit_warnings(&target, "", &content));
                delta.changes.push(AppliedPatchChange::Add {
                    path: normalize_display_path(&target),
                    content,
                    overwritten_content,
                });
                added.push(path.clone());
            }
            PatchHunk::DeleteFile { path } => {
                let target =
                    resolve_patch_path(cwd, path).map_err(|error| patch_failure(error, &delta))?;
                if target.is_dir() {
                    return Err(patch_failure(
                        AppError::BadRequest(format!(
                            "delete file target is a directory: {}",
                            target.to_string_lossy()
                        )),
                        &delta,
                    ));
                }
                let content = match fs::read_to_string(&target) {
                    Ok(content) => Some(content),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(_) => {
                        delta.exact = false;
                        None
                    }
                };
                if content.is_none() {
                    delta.exact = false;
                }
                if let Err(error) = fs::remove_file(&target) {
                    return Err(patch_failure(error.into(), &delta));
                }
                delta.changes.push(AppliedPatchChange::Delete {
                    path: normalize_display_path(&target),
                    content,
                });
                deleted.push(path.clone());
            }
            PatchHunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                let source =
                    resolve_patch_path(cwd, path).map_err(|error| patch_failure(error, &delta))?;
                if source.is_dir() {
                    return Err(patch_failure(
                        AppError::BadRequest(format!(
                            "update file target is a directory: {}",
                            source.to_string_lossy()
                        )),
                        &delta,
                    ));
                }
                let original = fs::read_to_string(&source)
                    .map_err(|error| patch_failure(error.into(), &delta))?;
                let updated = apply_update_chunks(&original, chunks, &source)
                    .map_err(|error| patch_failure(error, &delta))?;
                warnings.extend(collect_edit_warnings(&source, &original, &updated));
                if let Some(move_path) = move_path {
                    let target = resolve_patch_path(cwd, move_path)
                        .map_err(|error| patch_failure(error, &delta))?;
                    let overwritten_move_content = match fs::read_to_string(&target) {
                        Ok(content) => Some(content),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(_) => {
                            delta.exact = false;
                            None
                        }
                    };
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|error| patch_failure(error.into(), &delta))?;
                    }
                    if let Err(error) = fs::write(&target, &updated) {
                        delta.exact = false;
                        return Err(patch_failure(error.into(), &delta));
                    }
                    let pending_index = delta.changes.len();
                    delta.changes.push(AppliedPatchChange::Add {
                        path: normalize_display_path(&target),
                        content: updated.clone(),
                        overwritten_content: overwritten_move_content.clone(),
                    });
                    if let Err(error) = fs::remove_file(&source) {
                        return Err(patch_failure(error.into(), &delta));
                    }
                    delta.changes[pending_index] = AppliedPatchChange::Update {
                        path: normalize_display_path(&source),
                        move_path: Some(normalize_display_path(&target)),
                        old_content: original,
                        new_content: updated,
                        overwritten_move_content,
                    };
                    modified.push(move_path.clone());
                } else {
                    if let Err(error) = fs::write(&source, &updated) {
                        delta.exact = false;
                        return Err(patch_failure(error.into(), &delta));
                    }
                    delta.changes.push(AppliedPatchChange::Update {
                        path: normalize_display_path(&source),
                        move_path: None,
                        old_content: original,
                        new_content: updated,
                        overwritten_move_content: None,
                    });
                    modified.push(path.clone());
                }
            }
        }
    }

    Ok(ApplyPatchOutcome {
        summary: PatchSummary {
            added,
            modified,
            deleted,
        },
        delta,
        warnings,
    })
}

fn patch_failure(error: AppError, delta: &AppliedPatchDelta) -> ApplyPatchFailure {
    ApplyPatchFailure {
        message: error.to_string(),
        delta: delta.clone(),
    }
}

fn patch_delta_for_model(delta: &AppliedPatchDelta) -> Value {
    let changes = delta
        .changes
        .iter()
        .map(|change| match change {
            AppliedPatchChange::Add {
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
            AppliedPatchChange::Delete { path, content } => json!({
                "kind": "delete",
                "path": path,
                "contentAvailable": content.is_some(),
                "contentBytes": content.as_ref().map(String::len)
            }),
            AppliedPatchChange::Update {
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
        let context_start = if let Some(context) = &chunk.change_context {
            let Some(index) = find_context_line(&lines, context, cursor) else {
                return Err(AppError::BadRequest(format!(
                    "failed to find @@ context in {}: {}",
                    path.to_string_lossy(),
                    context
                )));
            };
            Some(index + 1)
        } else {
            None
        };
        let search_start = context_start.unwrap_or(cursor);
        if chunk.old_lines.is_empty() {
            let insert_at = if chunk.is_end_of_file {
                lines.len()
            } else {
                search_start.min(lines.len())
            };
            lines.splice(insert_at..insert_at, chunk.new_lines.clone());
            cursor = insert_at + chunk.new_lines.len();
            continue;
        }

        let matches = if chunk.change_context.is_some() {
            find_line_sequence_matches(&lines, &chunk.old_lines, search_start, chunk.is_end_of_file)
        } else {
            let primary = find_line_sequence_matches(
                &lines,
                &chunk.old_lines,
                search_start,
                chunk.is_end_of_file,
            );
            if primary.is_empty() && search_start != 0 {
                find_line_sequence_matches(&lines, &chunk.old_lines, 0, chunk.is_end_of_file)
            } else {
                primary
            }
        };
        let Some(found) = matches.first().copied() else {
            return Err(AppError::BadRequest(format!(
                "failed to find expected lines in {}:\n{}",
                path.to_string_lossy(),
                chunk.old_lines.join("\n")
            )));
        };
        if matches.len() > 1 {
            return Err(AppError::BadRequest(format!(
                "ambiguous patch hunk in {}: expected lines matched {} locations at lines {}. Add a unique @@ context or include more surrounding unchanged lines.\n{}",
                path.to_string_lossy(),
                matches.len(),
                format_line_candidates(&lines, &matches),
                chunk.old_lines.join("\n")
            )));
        }
        let end = found + chunk.old_lines.len();
        lines.splice(found..end, chunk.new_lines.clone());
        cursor = found + chunk.new_lines.len();
    }

    Ok(format!("{}\n", lines.join("\n")))
}

fn apply_multi_edit_file(
    target: &Path,
    args: &MultiEditFileArgs,
) -> Result<ApplyPatchOutcome, ApplyPatchFailure> {
    let delta = AppliedPatchDelta::default();
    if target.is_dir() {
        return Err(patch_failure(
            AppError::BadRequest(format!(
                "multi_edit_file target is a directory: {}",
                target.to_string_lossy()
            )),
            &delta,
        ));
    }
    if args.edits.is_empty() {
        return Err(patch_failure(
            AppError::BadRequest("edits cannot be empty".to_string()),
            &delta,
        ));
    }

    let original =
        fs::read_to_string(target).map_err(|error| patch_failure(error.into(), &delta))?;
    let line_endings = detect_text_line_endings(&original);
    let original_lf = normalize_to_lf(&original);
    let updated_lf = apply_multi_edits_to_lf_content(&original_lf, &args.edits, target)
        .map_err(|error| patch_failure(error, &delta))?;

    if updated_lf == original_lf {
        return Err(patch_failure(
            AppError::BadRequest("multi_edit_file produced no changes".to_string()),
            &delta,
        ));
    }

    let updated = restore_line_endings(&updated_lf, line_endings);
    let warnings = collect_edit_warnings(target, &original, &updated);
    fs::write(target, &updated).map_err(|error| {
        let mut failed_delta = delta.clone();
        failed_delta.exact = false;
        patch_failure(error.into(), &failed_delta)
    })?;

    let mut committed_delta = AppliedPatchDelta::default();
    committed_delta.changes.push(AppliedPatchChange::Update {
        path: normalize_display_path(target),
        move_path: None,
        old_content: original,
        new_content: updated,
        overwritten_move_content: None,
    });

    Ok(ApplyPatchOutcome {
        summary: PatchSummary {
            added: Vec::new(),
            modified: vec![args.path.clone()],
            deleted: Vec::new(),
        },
        delta: committed_delta,
        warnings,
    })
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
                "edit {} for {} has empty old_string; use apply_patch or write tooling for insert-only changes",
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

fn multi_edit_preview(args: &MultiEditFileArgs) -> String {
    let changes = args
        .edits
        .iter()
        .enumerate()
        .map(|(index, edit)| {
            format!(
                "{}. oldBytes={} newBytes={} replaceAll={} startLine={}",
                index + 1,
                edit.old_string.len(),
                edit.new_string.len(),
                edit.replace_all,
                edit.start_line
                    .map(|line| line.to_string())
                    .unwrap_or_else(|| "none".to_string())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("multi_edit_file {}\n{}", args.path, changes)
}

fn multi_edit_preview_changes(args: &MultiEditFileArgs) -> Value {
    json!({
        "kind": "multiEdit",
        "path": args.path,
        "edits": args.edits.iter().map(|edit| json!({
            "oldBytes": edit.old_string.len(),
            "newBytes": edit.new_string.len(),
            "replaceAll": edit.replace_all,
            "startLine": edit.start_line
        })).collect::<Vec<_>>()
    })
}

fn find_context_line(lines: &[String], context: &str, start: usize) -> Option<usize> {
    let needle = vec![context.to_string()];
    find_line_sequence(lines, &needle, start, false)
        .or_else(|| find_line_sequence(lines, &needle, 0, false))
}

fn find_line_sequence(
    lines: &[String],
    needle: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    find_line_sequence_matches(lines, needle, start, eof)
        .first()
        .copied()
}

fn find_line_sequence_matches(
    lines: &[String],
    needle: &[String],
    start: usize,
    eof: bool,
) -> Vec<usize> {
    if needle.is_empty() {
        return vec![start.min(lines.len())];
    }
    if needle.len() > lines.len() {
        return Vec::new();
    }
    let max_start = lines.len() - needle.len();
    let search_start = if eof {
        max_start
    } else if start > max_start {
        return Vec::new();
    } else {
        start
    };
    for matcher in [
        line_matches_exact as fn(&str, &str) -> bool,
        line_matches_trim_end,
        line_matches_trim,
        line_matches_normalized,
    ] {
        let mut matches = Vec::new();
        let mut seen = BTreeSet::new();
        for index in search_start..=max_start {
            if sequence_matches(lines, needle, index, matcher) {
                matches.push(index);
                seen.insert(index);
            }
        }
        if eof && start <= max_start && search_start != start {
            for index in start..=max_start {
                if !seen.contains(&index) && sequence_matches(lines, needle, index, matcher) {
                    matches.push(index);
                }
            }
        }
        if !matches.is_empty() {
            return matches;
        }
    }
    Vec::new()
}

fn format_line_candidates(lines: &[String], matches: &[usize]) -> String {
    let mut rendered = matches
        .iter()
        .take(5)
        .map(|index| {
            let line_no = index + 1;
            let preview = lines
                .get(*index)
                .map(|line| line.trim())
                .unwrap_or_default();
            if preview.is_empty() {
                line_no.to_string()
            } else {
                format!("{line_no} (`{preview}`)")
            }
        })
        .collect::<Vec<_>>();
    if matches.len() > rendered.len() {
        rendered.push(format!("and {} more", matches.len() - rendered.len()));
    }
    rendered.join(", ")
}

fn sequence_matches(
    lines: &[String],
    needle: &[String],
    index: usize,
    matcher: fn(&str, &str) -> bool,
) -> bool {
    needle
        .iter()
        .enumerate()
        .all(|(offset, expected)| matcher(&lines[index + offset], expected))
}

fn line_matches_exact(left: &str, right: &str) -> bool {
    left == right
}

fn line_matches_trim_end(left: &str, right: &str) -> bool {
    left.trim_end() == right.trim_end()
}

fn line_matches_trim(left: &str, right: &str) -> bool {
    left.trim() == right.trim()
}

fn line_matches_normalized(left: &str, right: &str) -> bool {
    normalize_patch_match_text(left) == normalize_patch_match_text(right)
}

fn normalize_patch_match_text(input: &str) -> String {
    input
        .trim()
        .chars()
        .map(|ch| match ch {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
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
    format!("{}\n", lines.join("\n"))
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
    fn prepend_path_entries_puts_bundled_paths_first() {
        let bundled = PathBuf::from("/app/tools");
        let existing_a = PathBuf::from("/usr/bin");
        let existing_b = PathBuf::from("/bin");
        let existing_path = env::join_paths([existing_a.clone(), existing_b.clone()])
            .expect("join existing test path");

        let updated = prepend_path_entries(
            std::slice::from_ref(&bundled),
            Some(existing_path.as_os_str()),
        )
        .expect("join updated test path");
        let paths = env::split_paths(&updated).collect::<Vec<_>>();

        assert_eq!(paths, vec![bundled, existing_a, existing_b]);
    }

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
    fn builtin_cwd_outside_allowed_dirs_returns_choices() {
        let temp_root = std::env::temp_dir().join(format!("mcp-gateway-{}", Uuid::new_v4()));
        let allowed = temp_root.join("allowed");
        let outside = temp_root.join("outside");
        fs::create_dir_all(&allowed).expect("create allowed test dir");
        fs::create_dir_all(&outside).expect("create outside test dir");

        let mut skills = SkillsConfig::default();
        skills.policy.path_guard.enabled = false;
        skills.policy.path_guard.whitelist_dirs = vec![allowed.to_string_lossy().to_string()];
        skills.policy.path_guard.on_violation = SkillPolicyAction::Allow;

        let outside_cwd = outside.to_string_lossy().to_string();
        let result = resolve_builtin_cwd(BuiltinTool::ShellCommand, &skills, Some(&outside_cwd))
            .expect_err("outside cwd should be rejected");

        assert!(result.is_error);
        assert_eq!(result.structured["code"], "InvalidCwd");
        assert_eq!(
            result.structured["message"],
            "cwd must be inside one configured allowed directory"
        );
        assert_eq!(
            result.structured["allowedDirectories"][0],
            normalize_display_path(&normalize_root_path(allowed.clone()))
        );
        assert_eq!(
            result.structured["resolvedCwd"],
            normalize_display_path(&normalize_root_path(outside))
        );
        assert!(result.text.contains("Allowed directories:"));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn builtin_cwd_missing_with_multiple_allowed_dirs_returns_choices() {
        let temp_root = std::env::temp_dir().join(format!("mcp-gateway-{}", Uuid::new_v4()));
        let first = temp_root.join("first");
        let second = temp_root.join("second");
        fs::create_dir_all(&first).expect("create first test dir");
        fs::create_dir_all(&second).expect("create second test dir");

        let mut skills = SkillsConfig::default();
        skills.policy.path_guard.whitelist_dirs = vec![
            first.to_string_lossy().to_string(),
            second.to_string_lossy().to_string(),
        ];

        let result = resolve_builtin_cwd(BuiltinTool::ShellCommand, &skills, None)
            .expect_err("ambiguous cwd should be rejected");

        assert!(result.is_error);
        assert_eq!(result.structured["code"], "InvalidCwd");
        assert_eq!(
            result.structured["message"],
            "cwd is required because multiple allowed directories are configured; ask the user which directory to operate in"
        );
        assert_eq!(
            result.structured["allowedDirectories"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(result.structured["requestedCwd"].is_null());

        let _ = fs::remove_dir_all(temp_root);
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
        assert!(text.contains("删除或禁用"));
    }

    #[test]
    fn tool_args_require_exec_not_legacy_cmd() {
        let args = decode_tool_args::<BuiltinShellArgs>(&json!({
            "exec": "Get-ChildItem -Name",
            "cwd": "D:/workspace"
        }))
        .expect("exec argument should decode");
        assert_eq!(args.exec, "Get-ChildItem -Name");

        let error = decode_tool_args::<BuiltinShellArgs>(&json!({
            "cmd": "Get-ChildItem -Name",
            "cwd": "D:/workspace"
        }))
        .expect_err("legacy cmd argument should not decode");
        assert!(error.message().contains("missing field `exec`"));
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

        let error = execute_skill_command(&mut command, timeout_ms, 4096, false, None, None)
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
    fn builtin_tool_definitions_include_shell_command_description() {
        let os = current_os_label();
        let now = Utc::now().to_rfc3339();
        let tools = builtin_tool_definitions(os, &now, &BuiltinToolsConfig::default());
        let shell_tool = tools
            .iter()
            .find(|item| item.get("name").and_then(Value::as_str) == Some("shell_command"))
            .expect("shell command tool exists");
        let shell_description = shell_tool
            .get("description")
            .and_then(Value::as_str)
            .expect("shell description");
        assert!(shell_description.contains("builtin://shell_command/SKILL.md"));
        assert!(shell_description.contains("MANDATORY BEFORE USE"));
        assert!(shell_description.contains("you MUST first read its full SKILL.md"));
        assert!(shell_description.contains("returned markdown content"));
        assert!(shell_description.contains("partial-read tricks"));
        assert!(shell_description.contains("prefer rg or rg --files"));
        assert!(shell_description.contains("node_modules"));
        assert!(!shell_description.contains("structuredContent.skillToken"));
        assert!(shell_description.contains("Front matter summary:"));
        assert!(shell_description.contains("SKILL.md URI:"));
        assert!(!shell_description.contains("Prefer fast discovery commands"));

        let patch_tool = tools
            .iter()
            .find(|item| item.get("name").and_then(Value::as_str) == Some("apply_patch"))
            .expect("apply patch tool exists");
        let patch_description = patch_tool
            .get("description")
            .and_then(Value::as_str)
            .expect("patch description");
        assert!(patch_description.contains("builtin://apply_patch/SKILL.md"));
        assert!(patch_description.contains("Front matter summary:"));
        assert!(!patch_description.contains("Minimal replacement:"));

        let names: Vec<&str> = tools
            .iter()
            .filter_map(|item| item.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            vec![
                "shell_command",
                "apply_patch",
                "multi_edit_file",
                "task-planning",
                "chrome-cdp",
                "chat-plus-adapter-debugger"
            ]
        );
    }

    #[test]
    fn builtin_skill_docs_are_served_from_embedded_skill_md() {
        let (tool, path) =
            builtin_skill_doc_read("Get-Content -Raw builtin://shell_command/SKILL.md")
                .expect("shell doc read");
        let shell = builtin_skill_doc_result(tool, "doc", path, "abc123".to_string(), false);
        assert!(!shell.is_error);
        assert!(shell.text.contains("# Shell Command"));
        assert!(shell
            .text
            .contains("## Global Search And Discovery Priority"));
        assert!(shell
            .text
            .contains("## Project And Workflow Navigation With Ripgrep"));
        assert!(shell.text.contains("rg --files"));
        assert!(shell.text.contains("Do not use `Get-ChildItem -Recurse`"));
        assert!(shell.text.contains("abc123"));
        assert_eq!(
            shell.structured.get("builtinSkill").and_then(Value::as_str),
            Some("shell_command")
        );
        assert!(shell.structured.get("skillToken").is_none());

        let (tool, path) = builtin_skill_doc_read("cat builtin://apply_patch/SKILL.md")
            .expect("apply_patch doc read");
        let patch = builtin_skill_doc_result(tool, "doc", path, "def456".to_string(), false);
        assert!(!patch.is_error);
        assert!(patch.text.contains("# Apply Patch"));
        assert!(patch.text.contains("*** Update File: path/to/file"));
        assert!(patch.text.contains("does not accept standard unified diff"));
        assert!(patch.text.contains("def456"));
        assert!(patch.structured.get("skillToken").is_none());

        let (tool, path) = builtin_skill_doc_read("cat builtin://multi_edit_file/SKILL.md")
            .expect("multi_edit_file doc read");
        let multi_edit = builtin_skill_doc_result(tool, "doc", path, "fed456".to_string(), false);
        assert!(!multi_edit.is_error);
        assert!(multi_edit.text.contains("# Multi Edit File"));
        assert!(multi_edit.text.contains("\"edits\""));
        assert!(multi_edit.text.contains("fed456"));
        assert!(multi_edit.structured.get("skillToken").is_none());

        let (tool, path) = builtin_skill_doc_read("cat builtin://task-planning/SKILL.md")
            .expect("task-planning doc read");
        let planning = builtin_skill_doc_result(tool, "doc", path, "plan123".to_string(), true);
        assert!(!planning.is_error);
        assert!(planning.text.contains("# Task Planning"));
        assert!(planning.text.contains("planningId"));
        assert!(planning.text.contains("plan123"));
        assert!(planning.structured.get("skillToken").is_none());

        let (tool, path) = builtin_skill_doc_read("cat builtin://chrome-cdp/SKILL.md")
            .expect("chrome-cdp doc read");
        let cdp = builtin_skill_doc_result(tool, "doc", path, "987abc".to_string(), false);
        assert!(!cdp.is_error);
        assert!(cdp.text.contains("# Chrome CDP"));
        assert!(cdp.text.contains("Chrome DevTools Protocol over WebSocket"));
        assert!(cdp.text.contains("netclear"));
        assert!(cdp.text.contains("CDP_PROFILE_MODE=persistent"));

        let (tool, path) = builtin_skill_doc_read(
            "Get-Content -Raw builtin://chat-plus-adapter-debugger/SKILL.md",
        )
        .expect("chat-plus adapter debugger doc read");
        let adapter = builtin_skill_doc_result(tool, "doc", path, "654fed".to_string(), false);
        assert!(!adapter.is_error);
        assert!(adapter.text.contains("# Chat Plus Adapter Debugger"));
        assert!(adapter.text.contains("decorateBubbles"));
        assert!(adapter.text.contains("capture start"));
        assert!(adapter.text.contains("network get <request-id>"));
        assert!(!adapter.text.contains("recorder-command.mjs"));
        assert!(!adapter
            .text
            .contains("mcp-gateway/crates/gateway-http/builtin-skills"));
        assert_eq!(
            adapter
                .structured
                .get("runtimeAssets")
                .and_then(|assets| assets.get("status"))
                .and_then(Value::as_str),
            Some("none")
        );
    }

    #[test]
    fn external_skill_docs_put_token_only_in_markdown_content() {
        let result = skill_doc_result(
            "demo_skill",
            "demo",
            "cat SKILL.md",
            "D:/skills/demo/SKILL.md".to_string(),
            "# Demo Skill\n".to_string(),
            "tok123".to_string(),
        );

        assert!(!result.is_error);
        assert!(result.text.contains("# Demo Skill"));
        assert!(result.text.contains("[skillToken]"));
        assert!(result.text.contains("tok123"));
        assert!(result.structured.get("skillToken").is_none());
    }

    #[test]
    fn non_documentation_calls_require_skill_md_hash_token() {
        let token = builtin_skill_token(BuiltinTool::ApplyPatch);
        assert_eq!(token.len(), 6);
        assert_eq!(
            token,
            skill_token_from_content(BUILTIN_APPLY_PATCH_SKILL_MD)
        );

        let missing = validate_skill_token_result(BuiltinTool::ApplyPatch.name(), &token, None)
            .expect("missing token should be rejected");
        assert!(missing.is_error);
        assert_eq!(
            missing.structured.get("code").and_then(Value::as_str),
            Some("SkillTokenRequired")
        );

        let invalid =
            validate_skill_token_result(BuiltinTool::ApplyPatch.name(), &token, Some("bad000"))
                .expect("invalid token should be rejected");
        assert!(invalid.text.contains("invalid skillToken"));

        let accepted =
            validate_skill_token_result(BuiltinTool::ApplyPatch.name(), &token, Some(&token));
        assert!(accepted.is_none());
    }

    #[test]
    fn builtin_chrome_cdp_command_parser_accepts_cdp_forms() {
        assert_eq!(
            parse_builtin_chrome_cdp_args("open https://example.com").expect("parse short"),
            vec!["open", "https://example.com"]
        );
        assert_eq!(
            parse_builtin_chrome_cdp_args("node cdp.mjs net api/chat").expect("parse node"),
            vec!["net", "api/chat"]
        );
        assert_eq!(
            parse_builtin_chrome_cdp_args("netget 123 --full").expect("parse netget"),
            vec!["netget", "123", "--full"]
        );
        assert!(parse_builtin_chrome_cdp_args("npm install unrelated-package").is_err());
        assert!(parse_builtin_chrome_cdp_args("npm install").is_err());
    }

    #[test]
    fn chat_plus_debug_command_parser_accepts_cdp_network_aliases() {
        match parse_chat_plus_debug_command("capture start").expect("parse capture") {
            Some(ChatPlusDebugCommand::CaptureStart) => {}
            other => panic!("unexpected capture command: {other:?}"),
        }

        match parse_chat_plus_debug_command("network search api chat").expect("parse search") {
            Some(ChatPlusDebugCommand::Cdp { command, .. }) => {
                assert_eq!(command, "net api chat");
            }
            other => panic!("unexpected network search command: {other:?}"),
        }

        match parse_chat_plus_debug_command("network get 123 --full").expect("parse get") {
            Some(ChatPlusDebugCommand::Cdp { command, .. }) => {
                assert_eq!(command, "netget 123 --full");
            }
            other => panic!("unexpected network get command: {other:?}"),
        }

        assert!(parse_chat_plus_debug_command("unsupported action")
            .expect("parse unsupported")
            .is_none());
    }

    #[test]
    fn apply_patch_updates_adds_and_deletes_files() {
        let sandbox = std::env::temp_dir().join(format!("gateway-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let update_path = sandbox.join("update.txt");
        let delete_path = sandbox.join("delete.txt");
        std::fs::write(&update_path, "alpha\nbeta\n").expect("write update");
        std::fs::write(&delete_path, "remove me\n").expect("write delete");

        let patch = "*** Begin Patch\n*** Update File: update.txt\n@@\n alpha\n-beta\n+gamma\n*** Add File: added.txt\n+new file\n*** Delete File: delete.txt\n*** End Patch"
            .to_string();
        let parsed = parse_apply_patch(&patch).expect("parse patch");
        let affected = patch_affected_paths(&parsed, &sandbox).expect("affected paths");
        assert_eq!(affected.len(), 3);

        let outcome = apply_parsed_patch(&parsed, &sandbox).expect("apply patch");
        assert_eq!(outcome.summary.added, vec!["added.txt"]);
        assert_eq!(outcome.summary.modified, vec!["update.txt"]);
        assert_eq!(outcome.summary.deleted, vec!["delete.txt"]);
        assert!(outcome.delta.exact);
        assert_eq!(outcome.delta.changes.len(), 3);
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

    #[test]
    fn apply_patch_rejects_empty_update() {
        let patch = "*** Begin Patch\n*** Update File: file.txt\n*** End Patch";
        let error = parse_apply_patch(patch).expect_err("empty update should be rejected");
        match error {
            AppError::BadRequest(message) => {
                assert!(message.contains("empty"));
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn apply_patch_accepts_heredoc_context_eof_and_fuzzy_match() {
        let sandbox = std::env::temp_dir().join(format!("gateway-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("update.txt");
        std::fs::write(
            &target,
            "intro\nfn target\n  value: old\u{2013}dash  \ntail\n",
        )
        .expect("write update");

        let patch = "<<'EOF'\n*** Begin Patch\n*** Update File: update.txt\n@@ fn target\n-  value: old-dash\n+  value: new-dash\n@@\n+done\n*** End of File\n*** End Patch\nEOF";
        let parsed = parse_apply_patch(patch).expect("parse patch");
        let outcome = apply_parsed_patch(&parsed, &sandbox).expect("apply patch");
        assert_eq!(outcome.summary.modified, vec!["update.txt"]);
        assert_eq!(
            std::fs::read_to_string(&target).expect("read update"),
            "intro\nfn target\n  value: new-dash\ntail\ndone\n"
        );

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn apply_patch_preserves_blank_lines_inside_update_hunks() {
        let sandbox = std::env::temp_dir().join(format!("gateway-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("update.txt");
        std::fs::write(&target, "alpha\n\nbeta\n").expect("write update");

        let patch = "*** Begin Patch\n*** Update File: update.txt\n@@\n alpha\n\n-beta\n+gamma\n*** End Patch";
        let parsed = parse_apply_patch(patch).expect("parse patch");
        let outcome = apply_parsed_patch(&parsed, &sandbox).expect("apply patch");
        assert_eq!(outcome.summary.modified, vec!["update.txt"]);
        assert_eq!(
            std::fs::read_to_string(&target).expect("read update"),
            "alpha\n\ngamma\n"
        );

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn apply_patch_rejects_missing_context_without_global_fallback() {
        let sandbox = std::env::temp_dir().join(format!("gateway-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("update.txt");
        std::fs::write(&target, "alpha\nbeta\n").expect("write update");

        let patch =
            "*** Begin Patch\n*** Update File: update.txt\n@@ missing-anchor\n-beta\n+gamma\n*** End Patch";
        let parsed = parse_apply_patch(patch).expect("parse patch");
        let failure = apply_parsed_patch(&parsed, &sandbox).expect_err("missing context fails");
        assert!(failure.message.contains("failed to find @@ context"));
        assert_eq!(
            std::fs::read_to_string(&target).expect("read update"),
            "alpha\nbeta\n"
        );

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn apply_patch_rejects_ambiguous_repeated_hunk_with_candidates() {
        let sandbox = std::env::temp_dir().join(format!("gateway-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("App.tsx");
        std::fs::write(
            &target,
            "<div>\n  <Code2 size={15} />\n</div>\n<div>\n  <Code2 size={15} />\n</div>\n",
        )
        .expect("write update");

        let patch = "*** Begin Patch\n*** Update File: App.tsx\n@@\n-  <Code2 size={15} />\n+  <Code2 size={16} />\n*** End Patch";
        let parsed = parse_apply_patch(patch).expect("parse patch");
        let failure = apply_parsed_patch(&parsed, &sandbox).expect_err("ambiguous hunk fails");
        assert!(failure.message.contains("ambiguous patch hunk"));
        assert!(failure.message.contains("lines 2"));
        assert!(failure.message.contains("5"));

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn apply_patch_preserves_template_expression_dollar() {
        let sandbox = std::env::temp_dir().join(format!("gateway-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("App.tsx");

        let patch = "*** Begin Patch\n*** Add File: App.tsx\n+const className = `${active ? \"on\" : \"off\"}`;\n*** End Patch";
        let parsed = parse_apply_patch(patch).expect("parse patch");
        let outcome = apply_parsed_patch(&parsed, &sandbox).expect("apply patch");
        assert!(outcome.warnings.is_empty());
        assert_eq!(
            std::fs::read_to_string(&target).expect("read update"),
            "const className = `${active ? \"on\" : \"off\"}`;\n"
        );

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn apply_patch_warns_on_escaped_template_expression() {
        let sandbox = std::env::temp_dir().join(format!("gateway-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");

        let patch = "*** Begin Patch\n*** Add File: App.tsx\n+const className = `\\${active}`;\n*** End Patch";
        let parsed = parse_apply_patch(patch).expect("parse patch");
        let outcome = apply_parsed_patch(&parsed, &sandbox).expect("apply patch");
        assert!(outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("`\\${`")));

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn apply_patch_add_file_overwrites_like_codex_and_records_delta() {
        let sandbox = std::env::temp_dir().join(format!("gateway-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("existing.txt");
        std::fs::write(&target, "old\n").expect("write existing");

        let patch = "*** Begin Patch\n*** Add File: existing.txt\n+new\n*** End Patch";
        let parsed = parse_apply_patch(patch).expect("parse patch");
        let outcome = apply_parsed_patch(&parsed, &sandbox).expect("apply patch");
        assert_eq!(outcome.summary.added, vec!["existing.txt"]);
        assert_eq!(
            std::fs::read_to_string(&target).expect("read existing"),
            "new\n"
        );
        match outcome.delta.changes.as_slice() {
            [AppliedPatchChange::Add {
                path,
                content,
                overwritten_content,
            }] => {
                assert!(Path::new(path).ends_with("existing.txt"));
                assert_eq!(content, "new\n");
                assert_eq!(overwritten_content.as_deref(), Some("old\n"));
            }
            other => panic!("unexpected delta: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn apply_patch_model_delta_omits_full_file_contents() {
        let delta = AppliedPatchDelta {
            exact: true,
            changes: vec![AppliedPatchChange::Update {
                path: "src/lib.rs".to_string(),
                move_path: None,
                old_content: "old\ncontent\n".to_string(),
                new_content: "new\ncontent\n".to_string(),
                overwritten_move_content: None,
            }],
        };

        let model_delta = patch_delta_for_model(&delta);
        let serialized = serde_json::to_string(&model_delta).expect("serialize model delta");
        assert!(serialized.contains("oldContentBytes"));
        assert!(serialized.contains("newContentBytes"));
        assert!(!serialized.contains("old\\ncontent"));
        assert!(!serialized.contains("new\\ncontent"));
    }

    #[test]
    fn apply_patch_failure_reports_committed_delta() {
        let sandbox = std::env::temp_dir().join(format!("gateway-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let patch = "*** Begin Patch\n*** Add File: added.txt\n+new file\n*** Delete File: missing.txt\n*** End Patch";
        let parsed = parse_apply_patch(patch).expect("parse patch");
        let failure = apply_parsed_patch(&parsed, &sandbox).expect_err("delete should fail");
        assert_eq!(failure.delta.changes.len(), 1);
        assert!(sandbox.join("added.txt").exists());

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn multi_edit_file_applies_multiple_edits_with_single_write() {
        let sandbox = std::env::temp_dir().join(format!("gateway-multi-edit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("update.txt");
        std::fs::write(&target, "alpha\nbeta\nbeta\ngamma\n").expect("write update");

        let args = MultiEditFileArgs {
            path: "update.txt".to_string(),
            cwd: None,
            skill_token: None,
            planning_id: None,
            edits: vec![
                MultiEditFileEdit {
                    old_string: "alpha".to_string(),
                    new_string: "ALPHA".to_string(),
                    replace_all: false,
                    start_line: None,
                },
                MultiEditFileEdit {
                    old_string: "beta".to_string(),
                    new_string: "BETA".to_string(),
                    replace_all: false,
                    start_line: Some(3),
                },
            ],
        };

        let outcome = apply_multi_edit_file(&target, &args).expect("apply multi edit");
        assert_eq!(outcome.summary.modified, vec!["update.txt"]);
        assert_eq!(
            std::fs::read_to_string(&target).expect("read update"),
            "ALPHA\nbeta\nBETA\ngamma\n"
        );
        match outcome.delta.changes.as_slice() {
            [AppliedPatchChange::Update {
                path,
                old_content,
                new_content,
                ..
            }] => {
                assert!(Path::new(path).ends_with("update.txt"));
                assert_eq!(old_content, "alpha\nbeta\nbeta\ngamma\n");
                assert_eq!(new_content, "ALPHA\nbeta\nBETA\ngamma\n");
            }
            other => panic!("unexpected delta: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn multi_edit_file_rejects_ambiguous_edit_without_writing() {
        let sandbox = std::env::temp_dir().join(format!("gateway-multi-edit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("update.txt");
        std::fs::write(&target, "alpha\nbeta\nbeta\n").expect("write update");

        let args = MultiEditFileArgs {
            path: "update.txt".to_string(),
            cwd: None,
            skill_token: None,
            planning_id: None,
            edits: vec![MultiEditFileEdit {
                old_string: "beta".to_string(),
                new_string: "BETA".to_string(),
                replace_all: false,
                start_line: None,
            }],
        };

        let failure =
            apply_multi_edit_file(&target, &args).expect_err("ambiguous edit should fail");
        assert!(failure.message.contains("found 2 matches"));
        assert_eq!(
            std::fs::read_to_string(&target).expect("read update"),
            "alpha\nbeta\nbeta\n"
        );

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn multi_edit_file_preserves_crlf_line_endings() {
        let sandbox = std::env::temp_dir().join(format!("gateway-multi-edit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("update.txt");
        std::fs::write(&target, "alpha\r\nbeta\r\n").expect("write update");

        let args = MultiEditFileArgs {
            path: "update.txt".to_string(),
            cwd: None,
            skill_token: None,
            planning_id: None,
            edits: vec![MultiEditFileEdit {
                old_string: "alpha\nbeta".to_string(),
                new_string: "ALPHA\nBETA".to_string(),
                replace_all: false,
                start_line: None,
            }],
        };

        apply_multi_edit_file(&target, &args).expect("apply multi edit");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read update"),
            "ALPHA\r\nBETA\r\n"
        );

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn multi_edit_file_warns_when_delimiters_become_unbalanced() {
        let sandbox = std::env::temp_dir().join(format!("gateway-multi-edit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox).expect("create sandbox");
        let target = sandbox.join("lib.rs");
        std::fs::write(&target, "fn outer() {\n    value();\n}\n").expect("write update");

        let args = MultiEditFileArgs {
            path: "lib.rs".to_string(),
            cwd: None,
            skill_token: None,
            planning_id: None,
            edits: vec![MultiEditFileEdit {
                old_string: "    value();\n}".to_string(),
                new_string: "    value();".to_string(),
                replace_all: false,
                start_line: None,
            }],
        };

        let outcome = apply_multi_edit_file(&target, &args).expect("apply multi edit");
        assert!(outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("unbalanced delimiters")));

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn disabled_builtin_tool_not_in_tool_definitions() {
        let os = "Windows";
        let now = "2024-01-01T00:00:00Z";
        let cfg = BuiltinToolsConfig {
            shell_command: false,
            apply_patch: true,
            multi_edit_file: true,
            task_planning: true,
            chrome_cdp: true,
            chat_plus_adapter_debugger: true,
        };
        let tools = builtin_tool_definitions(os, now, &cfg);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect();
        assert!(!names.contains(&"shell_command"));
        assert!(names.contains(&"apply_patch"));
        assert!(names.contains(&"multi_edit_file"));
    }

    #[test]
    fn all_disabled_except_one_returns_single_tool() {
        let os = "Windows";
        let now = "2024-01-01T00:00:00Z";
        let cfg = BuiltinToolsConfig {
            shell_command: false,
            apply_patch: false,
            multi_edit_file: false,
            task_planning: false,
            chrome_cdp: true,
            chat_plus_adapter_debugger: false,
        };
        let tools = builtin_tool_definitions(os, now, &cfg);
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].get("name").and_then(Value::as_str),
            Some("chrome-cdp")
        );
    }

    #[test]
    fn disabled_tool_rejected_in_execute_builtin_tool() {
        let all_enabled = BuiltinToolsConfig::default();
        assert_eq!(builtin_tools(&all_enabled).len(), 6);

        let all_disabled = BuiltinToolsConfig {
            shell_command: false,
            apply_patch: false,
            multi_edit_file: false,
            task_planning: false,
            chrome_cdp: false,
            chat_plus_adapter_debugger: false,
        };
        assert_eq!(builtin_tools(&all_disabled).len(), 0);
    }

    #[tokio::test]
    async fn task_planning_update_returns_stable_content_derived_id() {
        let service = SkillsService::new();
        let config = GatewayConfig::default();
        let token = builtin_skill_token(BuiltinTool::TaskPlanning);
        let args = json!({
            "action": "update",
            "explanation": "test",
            "plan": [
                {"step": "Inspect", "status": "completed"},
                {"step": "Implement", "status": "in_progress"}
            ],
            "skillToken": token
        });

        let first = service
            .execute_builtin_tool(
                &config,
                BuiltinTool::TaskPlanning,
                args.clone(),
                "session:a",
            )
            .await
            .expect("first planning update");
        let second = service
            .execute_builtin_tool(&config, BuiltinTool::TaskPlanning, args, "session:a")
            .await
            .expect("second planning update");
        let planning_id = first.structured["planning"]["planningId"]
            .as_str()
            .expect("planning id")
            .to_string();
        let status_update = service
            .execute_builtin_tool(
                &config,
                BuiltinTool::TaskPlanning,
                json!({
                    "action": "set_status",
                    "planningId": planning_id.clone(),
                    "status": "completed",
                    "skillToken": token
                }),
                "session:a",
            )
            .await
            .expect("completed planning update");

        let first_plan = &first.structured["planning"];
        let second_plan = &second.structured["planning"];
        let completed_plan = &status_update.structured["planning"];
        assert_eq!(first_plan["planningId"], second_plan["planningId"]);
        assert_eq!(first_plan["planningId"], completed_plan["planningId"]);
        assert_eq!(completed_plan["active"], false);
        assert_eq!(completed_plan["completed"], true);
    }

    #[tokio::test]
    async fn planning_gate_allows_reusing_id_across_tool_calls_until_completed() {
        let service = SkillsService::new();
        let config = GatewayConfig::default();
        let scope = "session:gate";

        let missing = service
            .check_planning_gate(&config, scope, BuiltinTool::ShellCommand, None)
            .await
            .expect("missing planning should be blocked");
        assert!(missing.is_error);
        assert_eq!(missing.structured["reason"], "missing planningId");

        let token = builtin_skill_token(BuiltinTool::TaskPlanning);
        let updated = service
            .execute_builtin_tool(
                &config,
                BuiltinTool::TaskPlanning,
                json!({
                    "action": "update",
                    "plan": [
                        {"step": "Inspect", "status": "in_progress"},
                        {"step": "Verify", "status": "pending"}
                    ],
                    "skillToken": token
                }),
                scope,
            )
            .await
            .expect("planning update");
        let planning_id = updated.structured["planning"]["planningId"]
            .as_str()
            .expect("planning id")
            .to_string();

        assert!(service
            .check_planning_gate(
                &config,
                scope,
                BuiltinTool::ShellCommand,
                Some(&planning_id),
            )
            .await
            .is_none());

        assert!(service
            .check_planning_gate(
                &config,
                scope,
                BuiltinTool::ShellCommand,
                Some(&planning_id),
            )
            .await
            .is_none());

        assert!(service
            .check_planning_gate(
                &config,
                "session:changed",
                BuiltinTool::ShellCommand,
                Some(&planning_id),
            )
            .await
            .is_none());

        let planning_reminder = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ShellCommand,
                Some("Get-ChildItem"),
            )
            .await
            .planning_reminder
            .expect("planning reminder");
        assert!(planning_reminder.contains("Current plan item #1"));
        assert!(planning_reminder.contains("\"action\":\"set_status\""));

        let reminder = tool_success_with_planning_reminder(
            "done".to_string(),
            json!({"status": "completed"}),
            PlanningSuccessHints {
                planning_reminder: Some(planning_reminder),
                shell_command_reminder: None,
            },
        );
        assert_eq!(reminder.text, "done");
        assert!(reminder.structured.get("planningUpdateRequired").is_none());
        assert!(reminder.structured["planningReminder"].is_string());
        assert!(reminder.structured.get("shellCommandReminder").is_none());

        let second_shell = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ShellCommand,
                Some("Get-ChildItem"),
            )
            .await;
        assert!(second_shell.shell_command_reminder.is_none());
        let third_shell = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ShellCommand,
                Some("Get-ChildItem"),
            )
            .await;
        assert!(third_shell.shell_command_reminder.is_some());
        let reset_by_rg = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ShellCommand,
                Some("rg --files"),
            )
            .await;
        assert!(reset_by_rg.shell_command_reminder.is_none());
        let after_rg_shell = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ShellCommand,
                Some("Get-ChildItem"),
            )
            .await;
        assert!(after_rg_shell.shell_command_reminder.is_none());
        let reset_by_patch = service
            .planning_success_hints(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ApplyPatch,
                None,
            )
            .await;
        assert!(reset_by_patch.shell_command_reminder.is_none());

        assert!(service
            .planning_edit_failure_reminder(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ApplyPatch,
            )
            .await
            .is_none());
        assert!(service
            .planning_edit_failure_reminder(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ApplyPatch,
            )
            .await
            .is_none());
        let edit_failure_reminder = service
            .planning_edit_failure_reminder(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::ApplyPatch,
            )
            .await
            .expect("third edit failure reminder");
        assert!(edit_failure_reminder.contains("apply_patch has failed 3 times"));
        assert!(service
            .planning_edit_failure_reminder(
                &config,
                "session:changed",
                Some(&planning_id),
                BuiltinTool::MultiEditFile,
            )
            .await
            .is_none());
        let edit_error = tool_error_with_edit_failure_reminder(
            "failed".to_string(),
            json!({"status": "failed"}),
            Some(edit_failure_reminder),
        );
        assert!(edit_error.structured["editFailureReminder"].is_string());

        let completed_first = service
            .execute_builtin_tool(
                &config,
                BuiltinTool::TaskPlanning,
                json!({
                    "action": "set_status",
                    "planningId": planning_id.clone(),
                    "status": "completed",
                    "skillToken": token
                }),
                scope,
            )
            .await
            .expect("complete current item");
        assert_eq!(
            completed_first.structured["planning"]["nextItem"]["item"],
            2
        );
        assert_eq!(
            completed_first.structured["planning"]["plan"][1]["status"],
            "in_progress"
        );
        assert!(service
            .check_planning_gate(
                &config,
                scope,
                BuiltinTool::ShellCommand,
                Some(&planning_id),
            )
            .await
            .is_none());

        service
            .execute_builtin_tool(
                &config,
                BuiltinTool::TaskPlanning,
                json!({
                    "action": "set_status",
                    "planningId": planning_id.clone(),
                    "status": "completed",
                    "skillToken": token
                }),
                scope,
            )
            .await
            .expect("complete final item");
        let closed = service
            .check_planning_gate(
                &config,
                scope,
                BuiltinTool::ShellCommand,
                Some(&planning_id),
            )
            .await
            .expect("completed planning id should be closed");
        assert_eq!(closed.structured["reason"], "unknown planningId");
    }
}
