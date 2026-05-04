use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use uuid::Uuid;

use gateway_core::AppError;

const MAX_OUTPUT_CHARS: usize = 1_000_000;
const EXEC_TIMEOUT_SECS: u64 = 60;
const ALLOWED_CWD_PREFIXES: &[&str] = &["/app", "/data"];
const DENY_TOKENS: &[&str] = &[
    "sudo",
    "su",
    "reboot",
    "shutdown",
    "mkfs",
    "dd",
    "poweroff",
];

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalTaskSnapshot {
    pub task_id: String,
    pub status: TerminalTaskStatus,
    pub command: String,
    pub cwd: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalTaskStatus {
    Running,
    Completed,
    Failed,
    Killed,
    Timeout,
}

#[derive(Debug)]
struct TerminalTask {
    snapshot: TerminalTaskSnapshot,
    child: Option<Arc<tokio::sync::Mutex<Child>>>,
}

#[derive(Clone, Default)]
pub struct TerminalService {
    tasks: Arc<RwLock<HashMap<String, Arc<RwLock<TerminalTask>>>>>,
}

impl TerminalService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn execute(&self, command: String, cwd: String) -> Result<TerminalTaskSnapshot, AppError> {
        validate_command(&command)?;
        validate_cwd(&cwd)?;

        let task_id = Uuid::new_v4().to_string();
        let mut child = Command::new("sh");
        child
            .arg("-lc")
            .arg(command.clone())
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut spawned = child.spawn().map_err(|err| AppError::Internal(format!("failed to spawn command: {err}")))?;
        let stdout = spawned.stdout.take().ok_or_else(|| AppError::Internal("stdout pipe unavailable".to_string()))?;
        let stderr = spawned.stderr.take().ok_or_else(|| AppError::Internal("stderr pipe unavailable".to_string()))?;

        let started_at = Utc::now();
        let child = Arc::new(tokio::sync::Mutex::new(spawned));
        let task = Arc::new(RwLock::new(TerminalTask {
            snapshot: TerminalTaskSnapshot {
                task_id: task_id.clone(),
                status: TerminalTaskStatus::Running,
                command,
                cwd,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                started_at,
                ended_at: None,
            },
            child: Some(child.clone()),
        }));

        self.tasks.write().await.insert(task_id.clone(), task.clone());
        tokio::spawn(stream_output(task.clone(), stdout, true));
        tokio::spawn(stream_output(task.clone(), stderr, false));
        tokio::spawn(wait_for_completion(task, child));

        Ok(self.get(&task_id).await?)
    }

    pub async fn get(&self, task_id: &str) -> Result<TerminalTaskSnapshot, AppError> {
        let task = self
            .tasks
            .read()
            .await
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound("terminal task not found".to_string()))?;
        let snapshot = task.read().await.snapshot.clone();
        Ok(snapshot)
    }

    pub async fn kill(&self, task_id: &str) -> Result<TerminalTaskSnapshot, AppError> {
        let task = self
            .tasks
            .read()
            .await
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound("terminal task not found".to_string()))?;

        let child = task.read().await.child.clone();
        if let Some(child) = child {
            let mut guard = child.lock().await;
            let _ = guard.kill().await;
        }

        let mut guard = task.write().await;
        guard.snapshot.status = TerminalTaskStatus::Killed;
        guard.snapshot.ended_at = Some(Utc::now());
        guard.child = None;
        Ok(guard.snapshot.clone())
    }
}

async fn stream_output(task: Arc<RwLock<TerminalTask>>, stream: impl tokio::io::AsyncRead + Unpin, is_stdout: bool) {
    let mut reader = BufReader::new(stream).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        let mut guard = task.write().await;
        let target = if is_stdout { &mut guard.snapshot.stdout } else { &mut guard.snapshot.stderr };
        if !target.is_empty() {
            target.push('\n');
        }
        target.push_str(&line);
        if target.len() > MAX_OUTPUT_CHARS {
            let truncate_to = target.len().saturating_sub(MAX_OUTPUT_CHARS);
            target.replace_range(..truncate_to, "");
        }
    }
}

async fn wait_for_completion(task: Arc<RwLock<TerminalTask>>, child: Arc<tokio::sync::Mutex<Child>>) {
    let wait = async {
        let mut guard = child.lock().await;
        guard.wait().await
    };

    match tokio::time::timeout(Duration::from_secs(EXEC_TIMEOUT_SECS), wait).await {
        Ok(Ok(status)) => {
            let mut guard = task.write().await;
            guard.snapshot.exit_code = status.code();
            guard.snapshot.status = if status.success() {
                TerminalTaskStatus::Completed
            } else {
                TerminalTaskStatus::Failed
            };
            guard.snapshot.ended_at = Some(Utc::now());
            guard.child = None;
        }
        Ok(Err(err)) => {
            let mut guard = task.write().await;
            guard.snapshot.status = TerminalTaskStatus::Failed;
            guard.snapshot.stderr.push_str(&format!("\nprocess wait failed: {err}"));
            guard.snapshot.ended_at = Some(Utc::now());
            guard.child = None;
        }
        Err(_) => {
            let mut child_guard = child.lock().await;
            let _ = child_guard.kill().await;
            let mut guard = task.write().await;
            guard.snapshot.status = TerminalTaskStatus::Timeout;
            guard.snapshot.ended_at = Some(Utc::now());
            guard.child = None;
        }
    }
}

fn validate_cwd(cwd: &str) -> Result<(), AppError> {
    let normalized = cwd.trim();
    if normalized.is_empty() {
        return Err(AppError::BadRequest("cwd cannot be empty".to_string()));
    }
    if !Path::new(normalized).is_absolute() {
        return Err(AppError::BadRequest("cwd must be an absolute path".to_string()));
    }
    if !ALLOWED_CWD_PREFIXES.iter().any(|prefix| normalized == *prefix || normalized.starts_with(&format!("{prefix}/"))) {
        return Err(AppError::BadRequest("cwd is outside allowed directories".to_string()));
    }
    Ok(())
}

fn validate_command(command: &str) -> Result<(), AppError> {
    let normalized = command.trim();
    if normalized.is_empty() {
        return Err(AppError::BadRequest("command cannot be empty".to_string()));
    }
    let lowered = normalized.to_ascii_lowercase();
    if DENY_TOKENS.iter().any(|token| lowered.split_whitespace().next() == Some(*token)) {
        return Err(AppError::BadRequest("command is blocked by terminal policy".to_string()));
    }
    if lowered.contains("rm -rf /") || lowered.contains("curl ") && lowered.contains("| bash") {
        return Err(AppError::BadRequest("command is blocked by terminal policy".to_string()));
    }
    Ok(())
}
