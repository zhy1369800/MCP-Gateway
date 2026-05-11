#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy)]
struct BundledTool {
    file_name: &'static str,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/bundled_tools.rs"));

const BUILTIN_SHELL_COMMAND_SKILL_MD: &str =
    include_str!("../../builtin-skills/shell_command/SKILL.md");
const BUILTIN_READ_FILE_SKILL_MD: &str = include_str!("../../builtin-skills/read_file/SKILL.md");
const BUILTIN_MULTI_EDIT_FILE_SKILL_MD: &str =
    include_str!("../../builtin-skills/multi_edit_file/SKILL.md");
const BUILTIN_TASK_PLANNING_SKILL_MD: &str =
    include_str!("../../builtin-skills/task-planning/SKILL.md");
const BUILTIN_CHROME_CDP_SKILL_MD: &str = include_str!("../../builtin-skills/chrome-cdp/SKILL.md");
const BUILTIN_CHROME_CDP_MJS: &str = include_str!("../../builtin-skills/chrome-cdp/scripts/cdp.mjs");
const BUILTIN_CHAT_PLUS_ADAPTER_DEBUGGER_SKILL_MD: &str =
    include_str!("../../builtin-skills/chat-plus-adapter-debugger/SKILL.md");
const BUILTIN_OFFICECLI_SKILL_MD: &str =
    include_str!("../../builtin-skills/officecli/SKILL.md");
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
    /// Per-path async mutexes for serializing concurrent builtin file writes.
    /// The outer std Mutex protects the table lookup/insert; the inner
    /// 	okio::sync::Mutex is held across await while a tool operates on the
    /// file so two concurrent multi_edit_file calls on the same path can't
    /// stomp on each other (lost update / old_string mismatch).
    file_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
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
    consecutive_multi_edit_file_failures: u32,
    consecutive_read_file_failures: u32,
    consecutive_chrome_cdp_failures: u32,
    officecli_pending_wps_cleanup: Option<OfficeCliPendingCleanup>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct OfficeCliPendingCleanup {
    file: String,
    created_at: Instant,
}

#[derive(Debug, Default)]
struct PlanningSuccessHints {
    planning_reminder: Option<String>,
    shell_command_reminder: Option<String>,
    read_failure_reminder: Option<String>,
    cdp_stuck_reminder: Option<String>,
    office_cli_post_create_reminder: Option<String>,
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
    pub reason_key: String,
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
struct ReadFileArgs {
    #[serde(alias = "filePath", alias = "file_path")]
    path: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    skill_token: Option<String>,
    #[serde(default)]
    planning_id: Option<String>,
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
    /// Optional list of paths the shell command is expected to write. When
    /// provided, the gateway serializes this call against other builtin tool
    /// calls (multi_edit_file, read_file, shell_command) targeting the same
    /// paths so they can't stomp on each other. If the command reads or
    /// writes other paths in addition, only the listed paths are protected.
    #[serde(default, alias = "writesPaths", alias = "writes_paths")]
    writes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MultiEditFileArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    edits: Vec<MultiEditFileEdit>,
    #[serde(default)]
    files: Vec<MultiEditFileSpec>,
    #[serde(default)]
    operations: Vec<MultiEditFileOperation>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    skill_token: Option<String>,
    #[serde(default)]
    planning_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MultiEditFileSpec {
    path: String,
    #[serde(default)]
    edits: Vec<MultiEditFileEdit>,
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum MultiEditFileOperation {
    #[serde(alias = "update")]
    Edit {
        path: String,
        edits: Vec<MultiEditFileEdit>,
    },
    Create {
        path: String,
        content: String,
        #[serde(default)]
        overwrite: bool,
    },
    Delete {
        path: String,
    },
    #[serde(alias = "rename")]
    Move {
        #[serde(alias = "fromPath", alias = "from_path")]
        from: String,
        #[serde(alias = "toPath", alias = "to_path")]
        to: String,
        #[serde(default)]
        overwrite: bool,
    },
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
    Confirm {
        reason: String,
        reason_key: String,
    },
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
    ReadFile,
    ShellCommand,
    MultiEditFile,
    TaskPlanning,
    ChromeCdp,
    ChatPlusAdapterDebugger,
    OfficeCli,
}

#[derive(Debug, Clone)]
struct ConfirmationMetadata {
    kind: String,
    cwd: String,
    affected_paths: Vec<String>,
    preview: String,
    reason_key: String,
}

#[derive(Debug)]
struct FileEditSummary {
    added: Vec<String>,
    modified: Vec<String>,
    deleted: Vec<String>,
    moved: Vec<String>,
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
    pub delta: Option<FileEditDelta>,
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
    delta: Option<FileEditDelta>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileEditDelta {
    changes: Vec<FileEditChange>,
    exact: bool,
}

impl Default for FileEditDelta {
    fn default() -> Self {
        Self {
            changes: Vec::new(),
            exact: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileEditChange {
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
struct FileEditFailure {
    message: String,
    delta: FileEditDelta,
    /// Accumulated warnings, notably rollback-after-commit-failure messages.
    warnings: Vec<String>,
}

#[derive(Debug)]
struct FileEditOutcome {
    summary: FileEditSummary,
    delta: FileEditDelta,
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


