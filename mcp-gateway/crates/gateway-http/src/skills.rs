use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use gateway_core::{
    AppError, ErrorCode, GatewayConfig, SkillCommandRule, SkillPolicyAction, SkillsConfig,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::sync::RwLock;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct SkillsService {
    confirmations: Arc<RwLock<HashMap<String, ConfirmationEntry>>>,
}

#[derive(Debug, Clone)]
struct ConfirmationEntry {
    request_fingerprint: String,
    record: SkillConfirmation,
}

#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillConfirmation {
    pub id: String,
    pub status: ConfirmationStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub skill: String,
    pub script: String,
    pub args: Vec<String>,
    pub command_preview: String,
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

#[derive(Debug)]
enum ConfirmationCheck {
    Missing,
    Approved,
    Pending,
    Rejected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillFrontmatter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
}

impl SkillsService {
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
                        "tools": tool_definitions(&discovered, &config.skills.roots),
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
        let guard = self.confirmations.read().await;
        let mut list = guard
            .values()
            .filter(|entry| entry.record.status == ConfirmationStatus::Pending)
            .map(|entry| entry.record.clone())
            .collect::<Vec<_>>();
        list.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        list
    }

    pub async fn approve_confirmation(&self, id: &str) -> Result<SkillConfirmation, AppError> {
        let mut guard = self.confirmations.write().await;
        let Some(entry) = guard.get_mut(id) else {
            return Err(AppError::NotFound("confirmation not found".to_string()));
        };
        if entry.record.status == ConfirmationStatus::Rejected {
            return Err(AppError::Conflict(
                "confirmation already rejected".to_string(),
            ));
        }
        entry.record.status = ConfirmationStatus::Approved;
        entry.record.updated_at = Utc::now();
        Ok(entry.record.clone())
    }

    pub async fn reject_confirmation(&self, id: &str) -> Result<SkillConfirmation, AppError> {
        let mut guard = self.confirmations.write().await;
        let Some(entry) = guard.get_mut(id) else {
            return Err(AppError::NotFound("confirmation not found".to_string()));
        };
        entry.record.status = ConfirmationStatus::Rejected;
        entry.record.updated_at = Utc::now();
        Ok(entry.record.clone())
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

        let skill_md_path = skill.path.join("SKILL.md");
        let policy = evaluate_policy(
            &config.skills,
            &program,
            &command_args,
            &skill_md_path,
            None,
        );
        let fingerprint = confirmation_fingerprint_for_command(&skill.skill, &command_preview);
        match policy {
            PolicyDecision::Deny(reason) => {
                let reason_text = reason.clone();
                return Ok(tool_error(
                    format!("command blocked by policy: {reason_text}"),
                    json!({
                        "status": "blocked",
                        "reason": reason,
                        "command": command_preview
                    }),
                ));
            }
            PolicyDecision::Confirm(reason) => {
                let outcome = self.consume_confirmation_by_fingerprint(&fingerprint).await;
                match outcome {
                    ConfirmationCheck::Missing | ConfirmationCheck::Pending => {
                        let confirmation = self
                            .create_confirmation(
                                &fingerprint,
                                &skill.skill,
                                &command_preview,
                                &tokens,
                                &command_preview,
                                &reason,
                            )
                            .await;
                        let reason_text = confirmation.reason.clone();
                        return Ok(tool_error(
                            format!("command requires user confirmation: {reason_text}"),
                            json!({
                                "status": "confirmation_required",
                                "confirmationId": confirmation.id,
                                "reason": confirmation.reason,
                                "command": confirmation.command_preview
                            }),
                        ));
                    }
                    ConfirmationCheck::Approved => {}
                    ConfirmationCheck::Rejected => {
                        return Ok(tool_error(
                            "user rejected confirmation request".to_string(),
                            json!({"status": "rejected"}),
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
        let output = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            command.output(),
        )
        .await
        .map_err(|_| AppError::Upstream(format!("command timed out after {timeout_ms}ms")))?
        .map_err(|error| AppError::Upstream(format!("failed to execute command: {error}")))?;

        let duration_ms = started.elapsed().as_millis() as u64;
        let disable_truncation = should_disable_output_truncation(&program, &command_args);
        let (stdout, stdout_truncated) = if disable_truncation {
            (String::from_utf8_lossy(&output.stdout).to_string(), false)
        } else {
            truncate_output(&output.stdout, max_output_bytes)
        };
        let (stderr, stderr_truncated) = if disable_truncation {
            (String::from_utf8_lossy(&output.stderr).to_string(), false)
        } else {
            truncate_output(&output.stderr, max_output_bytes)
        };
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

    async fn create_confirmation(
        &self,
        fingerprint: &str,
        skill: &str,
        script: &str,
        args: &[String],
        command_preview: &str,
        reason: &str,
    ) -> SkillConfirmation {
        {
            let guard = self.confirmations.read().await;
            if let Some(existing) = guard
                .values()
                .find(|entry| {
                    entry.request_fingerprint == fingerprint
                        && entry.record.status == ConfirmationStatus::Pending
                })
                .map(|entry| entry.record.clone())
            {
                return existing;
            }
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let record = SkillConfirmation {
            id: id.clone(),
            status: ConfirmationStatus::Pending,
            created_at: now,
            updated_at: now,
            skill: skill.to_string(),
            script: script.to_string(),
            args: args.to_vec(),
            command_preview: command_preview.to_string(),
            reason: reason.to_string(),
        };

        let mut guard = self.confirmations.write().await;
        guard.insert(
            id,
            ConfirmationEntry {
                request_fingerprint: fingerprint.to_string(),
                record: record.clone(),
            },
        );
        record
    }

    async fn consume_confirmation_by_fingerprint(&self, fingerprint: &str) -> ConfirmationCheck {
        let mut guard = self.confirmations.write().await;
        let Some((id, status)) = guard
            .iter()
            .find(|(_, entry)| entry.request_fingerprint == fingerprint)
            .map(|(id, entry)| (id.clone(), entry.record.status.clone()))
        else {
            return ConfirmationCheck::Missing;
        };

        match status {
            ConfirmationStatus::Approved => {
                guard.remove(&id);
                ConfirmationCheck::Approved
            }
            ConfirmationStatus::Pending => ConfirmationCheck::Pending,
            ConfirmationStatus::Rejected => {
                guard.remove(&id);
                ConfirmationCheck::Rejected
            }
        }
    }

    async fn discover_skills(
        &self,
        skills_config: &SkillsConfig,
    ) -> Result<Vec<DiscoveredSkill>, AppError> {
        let roots = skills_config.roots.clone();
        tokio::task::spawn_blocking(move || discover_skills_sync(&roots))
            .await
            .map_err(|error| AppError::Internal(format!("skills discovery join error: {error}")))?
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

fn tool_definitions(skills: &[DiscoveredSkill], roots: &[String]) -> Value {
    let bindings = build_skill_tool_bindings(skills);
    let now = Utc::now().to_rfc3339();
    let os = current_os_label();
    let root_hints = render_root_skill_md_hints(roots);

    Value::Array(
        bindings
            .into_iter()
            .map(|(tool_name, skill)| {
                let description =
                    render_skill_tool_description(skill, os, &now, &root_hints);
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
            })
            .collect(),
    )
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
    let raw = if !skill.frontmatter_name.trim().is_empty() {
        skill.frontmatter_name.trim()
    } else {
        skill.skill.trim()
    };
    sanitize_tool_name(raw)
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

fn render_skill_tool_description(
    skill: &DiscoveredSkill,
    os: &str,
    now: &str,
    root_hints: &str,
) -> String {
    let meta_description = if skill.description.trim().is_empty() {
        format!("Skill instructions for {}", skill.skill)
    } else {
        skill.description.trim().to_string()
    };
    format!(
        "To learn the complete usage of this skill, run `cmd` to read the full SKILL.md text. like `cat /.../SKILL.md` or `Get-Content D:/.../SKILL.md`. The `cmd` value should be one shell command string used either to read markdown files or run scripts. Current OS: {os}. Current datetime: {now}. Configured roots: {root_hints}. {meta_description} ."
    )
}

fn render_root_skill_md_hints(roots: &[String]) -> String {
    if roots.is_empty() {
        return "none".to_string();
    }

    roots
        .iter()
        .map(|root| {
            let path = PathBuf::from(root);
            let display_path = if path
                .file_name()
                .and_then(OsStr::to_str)
                .map(|name| name.eq_ignore_ascii_case("SKILL.md"))
                .unwrap_or(false)
            {
                path.parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| path.clone())
            } else {
                path
            };
            normalize_display_path(&display_path)
        })
        .collect::<Vec<_>>()
        .join(" | ")
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

    discovered.sort_by(|left, right| left.skill.to_lowercase().cmp(&right.skill.to_lowercase()));
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
    let (frontmatter_name, description) =
        parse_frontmatter_fields(&skill_md).unwrap_or_else(|_| (String::new(), String::new()));

    discovered.push(DiscoveredSkill {
        skill: dir_name.clone(),
        frontmatter_name,
        description,
        root: root_path.to_path_buf(),
        has_scripts: canonical_skill_dir.join("scripts").is_dir(),
        path: canonical_skill_dir,
    });
}

fn parse_frontmatter_fields(skill_md_path: &Path) -> Result<(String, String), AppError> {
    let content = std::fs::read_to_string(skill_md_path)?;
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return Ok((String::new(), String::new()));
    }

    let mut frontmatter_lines = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        frontmatter_lines.push(line.to_string());
    }

    let raw = frontmatter_lines.join("\n");
    if raw.trim().is_empty() {
        return Ok((String::new(), String::new()));
    }

    let frontmatter = serde_yaml_like_to_json(&raw)?;
    let parsed: SkillFrontmatter = serde_json::from_value(frontmatter).map_err(|error| {
        AppError::BadRequest(format!(
            "invalid frontmatter in {}: {error}",
            skill_md_path.display()
        ))
    })?;
    Ok((
        parsed.name.trim().to_string(),
        parsed.description.trim().to_string(),
    ))
}

fn serde_yaml_like_to_json(raw: &str) -> Result<Value, AppError> {
    let mut map = serde_json::Map::new();
    for line in raw.lines() {
        let Some((left, right)) = line.split_once(':') else {
            continue;
        };
        let key = left.trim();
        let value = right.trim().trim_matches('"').trim_matches('\'');
        if key.is_empty() {
            continue;
        }
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
    Ok(Value::Object(map))
}

fn shell_command_for_current_os(cmd: &str) -> (String, Vec<String>) {
    if cfg!(target_os = "windows") {
        (
            "powershell".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                cmd.to_string(),
            ],
        )
    } else {
        ("sh".to_string(), vec!["-lc".to_string(), cmd.to_string()])
    }
}

fn evaluate_policy(
    skills: &SkillsConfig,
    program: &str,
    command_args: &[String],
    script_path: &Path,
    script_text: Option<&str>,
) -> PolicyDecision {
    let invocations = collect_command_invocations(program, command_args, script_text);
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

fn confirmation_fingerprint_for_command(skill: &str, command: &str) -> String {
    format!(
        "{skill}|{}",
        command.trim().to_ascii_lowercase().replace('\n', " ")
    )
}

fn truncate_output(bytes: &[u8], max_bytes: usize) -> (String, bool) {
    let truncated = bytes.len() > max_bytes;
    let slice = if truncated {
        &bytes[..max_bytes]
    } else {
        bytes
    };
    (String::from_utf8_lossy(slice).to_string(), truncated)
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
        let decision = evaluate_policy(
            &skills,
            "sh",
            &[String::from("script.sh")],
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

        let decision = evaluate_policy(
            &skills,
            "python",
            &[
                String::from("tool.py"),
                blocked.to_string_lossy().to_string(),
            ],
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

        let decision = evaluate_policy(
            &skills,
            "python",
            &[String::from("safe.py"), String::from("--help")],
            Path::new("scripts/safe.py"),
            Some("print('ok')"),
        );

        match decision {
            PolicyDecision::Allow => {}
            _ => panic!("expected allow decision"),
        }
    }

    #[test]
    fn shell_wrapper_chain_is_blocked_by_default_rules() {
        let skills = SkillsConfig::default();
        let decision = evaluate_policy(
            &skills,
            "bash",
            &[String::from("-lc"), String::from("rm -rf /tmp/demo")],
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
    fn tool_definitions_use_metadata_name_and_cmd_schema() {
        let discovered = vec![
            DiscoveredSkill {
                skill: "alpha".to_string(),
                frontmatter_name: "Alpha Skill".to_string(),
                description: "A".to_string(),
                root: PathBuf::from("C:/skills"),
                path: PathBuf::from("C:/skills/alpha"),
                has_scripts: true,
            },
            DiscoveredSkill {
                skill: "beta".to_string(),
                frontmatter_name: "Beta Skill".to_string(),
                description: "B".to_string(),
                root: PathBuf::from("C:/skills"),
                path: PathBuf::from("C:/skills/beta"),
                has_scripts: false,
            },
        ];
        let roots = vec![
            "D:/Code_Save/Node_JS/browser-plugin/weather-skill".to_string(),
            "D:/Code_Save/Node_JS/skill-2".to_string(),
            "D:/Code_Save/Node_JS/skill-3".to_string(),
            "D:/Code_Save/Node_JS/skill-4".to_string(),
        ];

        let tools = tool_definitions(&discovered, &roots);
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
        assert!(description.contains("weather-skill"));

        let names = tools
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|item| item.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["alpha_skill", "beta_skill"]);
    }
}
