use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::Command;

use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State, WebSocketUpgrade};
use axum::extract::ws::{Message, WebSocket};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use gateway_core::{AppError, GatewayConfig, ServerConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use crate::ai_adapter::session::AiToolDef;

use crate::ai_adapter::session::AiSession;
use crate::response::{self, ApiResult};

use crate::state::AppState;
use crate::{ActivePlanSummary, SkillConfirmation, SkillSummary};

pub fn router(state: AppState, api_prefix: &str) -> Router {
    let prefix = api_prefix.trim_end_matches('/');
    Router::new()
        .route(&format!("{}/admin/health", prefix), get(get_health))
        .route(
            &format!("{}/admin/config", prefix),
            get(get_config).put(put_config),
        )
        .route(
            &format!("{}/admin/servers", prefix),
            get(get_servers).post(post_server),
        )
        .route(
            &format!("{}/admin/servers/:server_name", prefix),
            put(put_server).delete(delete_server),
        )
        .route(
            &format!("{}/admin/servers/:server_name/test", prefix),
            post(test_server),
        )
        .route(
            &format!("{}/admin/servers/:server_name/tools", prefix),
            get(get_server_tools),
        )
        .route(
            &format!("{}/admin/export/mcp-servers", prefix),
            get(export_mcp_servers_payload),
        )
        .route(
            &format!("{}/admin/terminal/ws", prefix),
            get(ws_terminal_handler),
        )
        .route(
            &format!("{}/admin/runtimes", prefix),
            get(get_server_runtimes),
        )
        .route(&format!("{}/admin/skills", prefix), get(get_skills))
        .route(
            &format!("{}/admin/skills/plans", prefix),
            get(get_active_plans),
        )
        .route(
            &format!("{}/admin/skills/plans/:planning_id", prefix),
            delete(delete_active_plan),
        )
        .route(
            &format!("{}/admin/skills/events", prefix),
            get(get_skill_events),
        )
        .route(
            &format!("{}/admin/skills/validate-root", prefix),
            post(validate_skill_root),
        )
        .route(
            &format!("{}/admin/skills/upload", prefix),
            post(upload_skill_root).layer(DefaultBodyLimit::max(5 * 1024 * 1024)),
        )
        .route(
            &format!("{}/admin/skills/directory", prefix),
            delete(delete_skill_directory),
        )
        .route(
            &format!("{}/admin/skills/confirmations", prefix),
            get(get_pending_skill_confirmations),
        )
        .route(
            &format!(
                "{}/admin/skills/confirmations/:confirmation_id/approve",
                prefix
            ),
            post(approve_skill_confirmation),
        )
        .route(
            &format!(
                "{}/admin/skills/confirmations/:confirmation_id/reject",
                prefix
            ),
            post(reject_skill_confirmation),
        )
        .route(
            &format!("{}/admin/ai-sessions", prefix),
            get(get_ai_sessions),
        )
        .route(
            &format!("{}/admin/ai-sessions/:session_id/rename", prefix),
            post(rename_ai_session),
        )
        .route(
            &format!("{}/admin/ai-sessions/:session_id/tools/:tool_name", prefix),
            put(toggle_ai_session_tool),
        )
        .route(
            &format!("{}/admin/ai-sessions/:session_id", prefix),
            delete(delete_ai_session),
        )
        .route(
            &format!("{}/admin/ai-sessions/:session_id/system-prompt", prefix),
            put(update_ai_session_system_prompt),
        )
        .route(
            &format!(
                "{}/admin/ai-sessions/:session_id/system-prompt-tool",
                prefix
            ),
            put(toggle_ai_session_system_prompt_tool),
        )
        .route(
            &format!("{}/admin/ai-sessions/:session_id/tool-ping", prefix),
            put(toggle_ai_session_tool_ping),
        )
        .with_state(state)
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthData {
    started_at: chrono::DateTime<chrono::Utc>,
    uptime_seconds: i64,
    mode: String,
    listen: String,
    server_count: usize,
    version: &'static str,
}

#[utoipa::path(
    get,
    path = "/{api_prefix}/admin/health",
    responses((status = 200, description = "Gateway health"))
)]
pub async fn get_health(State(state): State<AppState>) -> ApiResult<HealthData> {
    let cfg = state.config_service.get_config().await;
    Ok(response::ok(HealthData {
        started_at: state.started_at,
        uptime_seconds: (chrono::Utc::now() - state.started_at).num_seconds(),
        mode: cfg.mode.to_string(),
        listen: cfg.listen,
        server_count: cfg.servers.len(),
        version: env!("CARGO_PKG_VERSION"),
    }))
}

#[utoipa::path(
    get,
    path = "/{api_prefix}/admin/config",
    responses((status = 200, description = "Current config"))
)]
pub async fn get_config(State(state): State<AppState>) -> ApiResult<GatewayConfig> {
    let cfg = state.config_service.get_config().await;
    Ok(response::ok(cfg))
}

#[utoipa::path(
    put,
    path = "/{api_prefix}/admin/config",
    request_body = GatewayConfig,
    responses((status = 200, description = "Updated config"))
)]
pub async fn put_config(
    State(state): State<AppState>,
    Json(next_config): Json<GatewayConfig>,
) -> ApiResult<GatewayConfig> {
    let updated = state
        .config_service
        .replace(next_config)
        .await
        .map_err(response::err_response)?;
    state.process_manager.reset_pool().await;
    Ok(response::ok(updated))
}

#[utoipa::path(
    get,
    path = "/{api_prefix}/admin/servers",
    responses((status = 200, description = "Server list"))
)]
pub async fn get_servers(State(state): State<AppState>) -> ApiResult<Vec<ServerConfig>> {
    let cfg = state.config_service.get_config().await;
    Ok(response::ok(cfg.servers))
}

#[utoipa::path(
    post,
    path = "/{api_prefix}/admin/servers",
    request_body = ServerConfig,
    responses((status = 200, description = "Server created"))
)]
pub async fn post_server(
    State(state): State<AppState>,
    Json(server): Json<ServerConfig>,
) -> ApiResult<ServerConfig> {
    let server_name = server.name.clone();
    state
        .config_service
        .update(|current| {
            if current.servers.iter().any(|item| item.name == server_name) {
                return Err(AppError::Conflict(format!(
                    "server already exists: {server_name}"
                )));
            }
            let mut cfg = current.clone();
            cfg.servers.push(server.clone());
            Ok(cfg)
        })
        .await
        .map_err(response::err_response)?;

    state.process_manager.evict_server(&server_name).await;
    Ok(response::ok(server))
}

#[utoipa::path(
    put,
    path = "/{api_prefix}/admin/servers/{server_name}",
    request_body = ServerConfig,
    params(("server_name" = String, Path, description = "Server name")),
    responses((status = 200, description = "Server updated"))
)]
pub async fn put_server(
    State(state): State<AppState>,
    Path(server_name): Path<String>,
    Json(server): Json<ServerConfig>,
) -> ApiResult<ServerConfig> {
    let next = server.clone();
    state
        .config_service
        .update(|current| {
            let Some(index) = current
                .servers
                .iter()
                .position(|item| item.name == server_name)
            else {
                return Err(AppError::NotFound("server not found".to_string()));
            };

            let mut cfg = current.clone();
            let mut updated_server = next.clone();
            updated_server.name = server_name.clone();
            cfg.servers[index] = updated_server;
            Ok(cfg)
        })
        .await
        .map_err(response::err_response)?;

    state.process_manager.evict_server(&server_name).await;
    Ok(response::ok(server))
}

#[utoipa::path(
    delete,
    path = "/{api_prefix}/admin/servers/{server_name}",
    params(("server_name" = String, Path, description = "Server name")),
    responses((status = 200, description = "Server deleted"))
)]
pub async fn delete_server(
    State(state): State<AppState>,
    Path(server_name): Path<String>,
) -> ApiResult<Value> {
    state
        .config_service
        .update(|current| {
            let before = current.servers.len();
            let mut cfg = current.clone();
            cfg.servers.retain(|item| item.name != server_name);
            if cfg.servers.len() == before {
                return Err(AppError::NotFound("server not found".to_string()));
            }
            Ok(cfg)
        })
        .await
        .map_err(response::err_response)?;

    state.process_manager.evict_server(&server_name).await;
    Ok(response::ok(json!({"deleted": server_name})))
}

#[utoipa::path(
    post,
    path = "/{api_prefix}/admin/servers/{server_name}/test",
    params(("server_name" = String, Path, description = "Server name")),
    responses((status = 200, description = "Server test result"))
)]
pub async fn test_server(
    State(state): State<AppState>,
    Path(server_name): Path<String>,
) -> ApiResult<Value> {
    let cfg = state.config_service.get_config().await;
    let server = cfg
        .servers
        .iter()
        .find(|item| item.name == server_name)
        .cloned()
        .ok_or_else(|| {
            response::err_response(AppError::NotFound("server not found".to_string()))
        })?;

    let result = state
        .process_manager
        .test_server(&server, &cfg.defaults)
        .await
        .map_err(response::err_response)?;

    Ok(response::ok(result))
}

#[derive(Debug, Deserialize)]
pub struct ToolsQuery {
    refresh: Option<bool>,
}

#[utoipa::path(
    get,
    path = "/api/v2/admin/servers/{server_name}/tools",
    params(
        ("server_name" = String, Path, description = "Server name"),
        ("refresh" = Option<bool>, Query, description = "Force refresh from upstream")
    ),
    responses((status = 200, description = "Server tools"))
)]
pub async fn get_server_tools(
    State(state): State<AppState>,
    Path(server_name): Path<String>,
    Query(query): Query<ToolsQuery>,
) -> ApiResult<Value> {
    let cfg = state.config_service.get_config().await;
    let server = cfg
        .servers
        .iter()
        .find(|item| item.name == server_name)
        .cloned()
        .ok_or_else(|| {
            response::err_response(AppError::NotFound("server not found".to_string()))
        })?;

    let refresh = query.refresh.unwrap_or(true);
    let result = state
        .process_manager
        .list_tools(&server, &cfg.defaults, refresh)
        .await
        .map_err(response::err_response)?;

    Ok(response::ok(json!({"refresh": refresh, "result": result})))
}

pub async fn ws_terminal_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl axum::response::IntoResponse {
    let cwd = params.get("cwd").cloned();
    ws.on_upgrade(move |socket| handle_terminal_socket(socket, state, cwd))
}

async fn handle_terminal_socket(
    socket: WebSocket,
    _state: AppState,
    cwd: Option<String>,
) {
    use futures_util::{SinkExt, StreamExt};
    use std::io::Read;

    let pty_system = NativePtySystem::default();
    let pty_pair = match pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("Failed to open PTY: {:?}", e);
            return;
        }
    };

    #[cfg(target_os = "windows")]
    let mut cmd = CommandBuilder::new("powershell.exe");

    #[cfg(not(target_os = "windows"))]
    let mut cmd = {
        let has_coder_user = std::path::Path::new("/home/coder").exists();
        let is_root = std::env::var("USER").map(|u| u == "root").unwrap_or(false)
            || std::path::Path::new("/root").exists();

        if is_root && has_coder_user {
            let mut c = CommandBuilder::new("/bin/su");
            c.args(&["-s", "/bin/bash", "coder"]);
            c
        } else {
            CommandBuilder::new("/bin/bash")
        }
    };
    // 设置带路径的提示符格式：[user@host cwd]#
    cmd.env("PS1", r"\[\e[0;32m\][\u@\h \w]\$\[\e[0m\] ");
    if let Some(c) = cwd {
        cmd.cwd(c);
    }

    let _child = match pty_pair.slave.spawn_command(cmd) {
        Ok(child) => child,
        Err(e) => {
            eprintln!("Failed to spawn shell command in PTY: {:?}", e);
            return;
        }
    };

    let mut reader = match pty_pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to clone PTY reader: {:?}", e);
            return;
        }
    };
    let mut writer = match pty_pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to get PTY writer: {:?}", e);
            return;
        }
    };
    let master = pty_pair.master;

    let (pty_tx, mut pty_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
    let (ws_tx, mut ws_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);

    // 启动 PTY 同步阻塞读取线程
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    if pty_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
    });

    // 启动 PTY 同步阻塞写入线程
    std::thread::spawn(move || {
        use std::io::Write;
        while let Some(data) = ws_rx.blocking_recv() {
            if writer.write_all(&data).is_err() {
                break;
            }
            if writer.flush().is_err() {
                break;
            }
        }
    });

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // 异步任务一：读取 pty_rx，通过 websocket 发送给前端
    let mut read_task = tokio::spawn(async move {
        while let Some(bytes) = pty_rx.recv().await {
            if ws_sender.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
    });

    // 异步任务二：从 websocket 读前端的发送包，送入 ws_tx 或者控制 PTY 尺寸
    let ws_tx_clone = ws_tx.clone();
    let mut write_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                Message::Text(text) => {
                    if text.starts_with('{') {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                            if val["type"] == "resize" {
                                if let (Some(cols), Some(rows)) = (val["cols"].as_u64(), val["rows"].as_u64()) {
                                    let _ = master.resize(PtySize {
                                        rows: rows as u16,
                                        cols: cols as u16,
                                        pixel_width: 0,
                                        pixel_height: 0,
                                    });
                                    continue;
                                }
                            }
                        }
                    }
                    if ws_tx_clone.send(text.into_bytes()).await.is_err() {
                        break;
                    }
                }
                Message::Binary(bin) => {
                    if ws_tx_clone.send(bin.to_vec()).await.is_err() {
                        break;
                    }
                }
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = &mut read_task => {
            write_task.abort();
        }
        _ = &mut write_task => {
            read_task.abort();
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/admin/export/mcp-servers",
    responses((status = 200, description = "Export MCP server payload"))
)]
pub async fn export_mcp_servers_payload(State(state): State<AppState>) -> ApiResult<Value> {
    let cfg = state.config_service.get_config().await;
    let base_url = gateway_base_url(&cfg.listen).map_err(response::err_response)?;
    let transport_base = cfg
        .transport
        .streamable_http
        .base_path
        .trim_end_matches('/');

    let maybe_auth_header = if cfg.security.mcp.enabled {
        Some(json!({"Authorization": format!("Bearer {}", cfg.security.mcp.token)}))
    } else {
        None
    };

    let build_entry = |name: &str, server_path: &str| -> Value {
        let url = format!("{}{}/{}", base_url, transport_base, server_path);
        let mut entry = serde_json::Map::new();
        entry.insert("name".to_string(), Value::String(name.to_string()));
        entry.insert(
            "type".to_string(),
            Value::String("streamable-http".to_string()),
        );
        entry.insert("url".to_string(), Value::String(url));
        if let Some(ref h) = maybe_auth_header {
            entry.insert("headers".to_string(), h.clone());
        }
        Value::Object(entry)
    };

    let mut mcp_servers = serde_json::Map::new();
    for server in cfg.servers.iter().filter(|item| item.enabled) {
        mcp_servers.insert(
            server.name.clone(),
            build_entry(&server.display_name(), &server.name),
        );
    }

    mcp_servers.insert(
        cfg.skills.server_name.clone(),
        build_entry("External Skills MCP", &cfg.skills.server_name),
    );
    mcp_servers.insert(
        cfg.skills.builtin_server_name.clone(),
        build_entry("Built-in Skills MCP", &cfg.skills.builtin_server_name),
    );

    // AI Adapter 会话
    let sessions = state.ai_sessions.list_sessions().await;
    for session in &sessions {
        let encoded_name = percent_encoding::utf8_percent_encode(
            &session.name,
            percent_encoding::NON_ALPHANUMERIC,
        );
        mcp_servers.insert(
            session.name.clone(),
            build_entry(
                &format!("AI Adapter: {}", session.name),
                &encoded_name.to_string(),
            ),
        );
    }

    Ok(response::ok(
        json!({"mcpServers": Value::Object(mcp_servers)}),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v2/admin/skills",
    responses((status = 200, description = "Discovered skills"))
)]
pub async fn get_skills(State(state): State<AppState>) -> ApiResult<Vec<SkillSummary>> {
    let cfg = state.config_service.get_config().await;
    let skills = state
        .skills
        .list_skills_for_admin(&cfg)
        .await
        .map_err(response::err_response)?;
    Ok(response::ok(skills))
}

#[derive(Debug, Deserialize, Serialize, utoipa::ToSchema)]
pub struct SkillEventsQuery {
    after: Option<u64>,
}


#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillDirectoryValidation {
    exists: bool,
    is_dir: bool,
    has_skill_md: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillUploadResult {
    path: String,
    exists: bool,
    is_dir: bool,
    has_skill_md: bool,
    uploaded_files: usize,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidateSkillRootRequest {
    path: String,
}

#[utoipa::path(
    post,
    path = "/api/v2/admin/skills/validate-root",
    request_body = ValidateSkillRootRequest,
    responses((status = 200, description = "Skill root validation result", body = SkillDirectoryValidation))
)]
pub async fn validate_skill_root(
    State(_state): State<AppState>,
    Json(payload): Json<ValidateSkillRootRequest>,
) -> ApiResult<SkillDirectoryValidation> {
    let path = PathBuf::from(payload.path.trim());
    if payload.path.trim().is_empty() {
        return Ok(response::ok(SkillDirectoryValidation {
            exists: false,
            is_dir: false,
            has_skill_md: false,
        }));
    }

    let metadata = tokio::fs::metadata(&path).await.ok();
    let exists = metadata.is_some();
    let is_dir = metadata.as_ref().is_some_and(|meta| meta.is_dir());
    let has_skill_md = if is_dir {
        tokio::fs::metadata(path.join("SKILL.md")).await.is_ok()
    } else {
        false
    };

    Ok(response::ok(SkillDirectoryValidation {
        exists,
        is_dir,
        has_skill_md,
    }))
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSkillDirectoryRequest {
    pub path: String,
}

#[utoipa::path(
    delete,
    path = "/api/v2/admin/skills/directory",
    request_body = DeleteSkillDirectoryRequest,
    responses(
        (status = 200, description = "Skill directory deleted"),
        (status = 400, description = "Invalid path or permission denied")
    )
)]
pub async fn delete_skill_directory(
    State(_state): State<AppState>,
    Json(payload): Json<DeleteSkillDirectoryRequest>,
) -> ApiResult<Value> {
    let path_str = payload.path.trim();
    if path_str.is_empty() {
        return Err(response::err_response(AppError::BadRequest(
            "path is required".to_string(),
        )));
    }

    let target_path = PathBuf::from(path_str);
    let canonical = target_path.canonicalize().unwrap_or_else(|_| target_path.clone());
    
    let safe_root_str = std::env::var("MCP_SKILLS_ROOT").unwrap_or_else(|_| "/data/skills".to_string());
    let safe_root = std::path::Path::new(&safe_root_str);

    if !canonical.starts_with(safe_root) {
        return Err(response::err_response(AppError::BadRequest(format!(
            "安全校验失败：只允许删除技能根目录({})之下的子文件夹",
            safe_root.display()
        ))));
    }
    if canonical == safe_root {
        return Err(response::err_response(AppError::BadRequest(format!(
            "安全校验失败：禁止删除技能根目录自身({})",
            safe_root.display()
        ))));
    }

    if canonical.exists() {
        if canonical.is_dir() {
            tokio::fs::remove_dir_all(&canonical)
                .await
                .map_err(|err| response::err_response(AppError::Internal(format!("删除技能目录失败：{}", err))))?;
        } else {
            tokio::fs::remove_file(&canonical)
                .await
                .map_err(|err| response::err_response(AppError::Internal(format!("删除技能文件失败：{}", err))))?;
        }
    }

    Ok(response::ok(json!({
        "path": path_str,
        "deleted": true
    })))
}

#[utoipa::path(
    post,
    path = "/api/v2/admin/skills/upload",
    responses((status = 200, description = "Upload local skill directory to remote root", body = SkillUploadResult))
)]
pub async fn upload_skill_root(
    State(_state): State<AppState>,
    mut multipart: Multipart,
) -> ApiResult<SkillUploadResult> {
    let mut target_root: Option<String> = None;
    let mut uploaded_files = 0usize;
    let mut skill_name: Option<String> = None;
    let mut uploaded_root: Option<PathBuf> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| {
            let err_str = err.to_string();
            let friendly_msg = if err_str.contains("limit") || err_str.contains("large") || err_str.contains("multipart") {
                "上传的技能文件夹总大小超过了 5MB 的限制，请删除无用文件（如 node_modules、.git 等目录）后重试。".to_string()
            } else {
                format!("解析上传数据失败：{}", err_str)
            };
            response::err_response(AppError::BadRequest(friendly_msg))
        })?
    {
        let Some(name) = field.name().map(str::to_string) else {
            continue;
        };

        if name == "targetRoot" {
            let value = field
                .text()
                .await
                .map_err(|err| response::err_response(AppError::BadRequest(format!("invalid targetRoot field: {err}"))))?;
            target_root = Some(value.trim().to_string());
            continue;
        }

        if name != "files" {
            continue;
        }

        let file_name = field.file_name().map(str::to_string).ok_or_else(|| {
            response::err_response(AppError::BadRequest("uploaded file is missing a relative path".to_string()))
        })?;
        let target_root = target_root.clone().ok_or_else(|| {
            response::err_response(AppError::BadRequest("targetRoot is required before files".to_string()))
        })?;
        let rel_path = sanitize_upload_relative_path(&file_name)
            .map_err(response::err_response)?;
        let mut components = rel_path.components();
        let Some(first) = components.next() else {
            return Err(response::err_response(AppError::BadRequest(
                "uploaded file path cannot be empty".to_string(),
            )));
        };
        let first = first.as_os_str().to_string_lossy().to_string();
        if first.trim().is_empty() {
            return Err(response::err_response(AppError::BadRequest(
                "uploaded file path cannot have empty root segment".to_string(),
            )));
        }
        if let Some(existing) = &skill_name {
            if existing != &first {
                return Err(response::err_response(AppError::BadRequest(
                    "please upload files from a single top-level folder".to_string(),
                )));
            }
        } else {
            skill_name = Some(first.clone());
        }

        let skill_root = PathBuf::from(&target_root).join(&first);
        uploaded_root = Some(skill_root.clone());
        let relative_inside_skill = components.as_path();
        let target_path = if relative_inside_skill.as_os_str().is_empty() {
            skill_root.join("SKILL.md")
        } else {
            skill_root.join(relative_inside_skill)
        };

        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| response::err_response(AppError::Internal(format!("failed to create upload directory: {err}"))))?;
        }

        let bytes = field
            .bytes()
            .await
            .map_err(|err| {
                let err_str = err.to_string();
                let friendly_msg = if err_str.contains("limit") || err_str.contains("large") || err_str.contains("multipart") {
                    "上传的技能文件夹总大小超过了 5MB 的限制，请删除无用文件（如 node_modules、.git 等目录）后重试。".to_string()
                } else {
                    format!("读取上传文件失败：{}", err_str)
                };
                response::err_response(AppError::BadRequest(friendly_msg))
            })?;
        tokio::fs::write(&target_path, bytes)
            .await
            .map_err(|err| response::err_response(AppError::Internal(format!("failed to write uploaded file: {err}"))))?;
        uploaded_files += 1;
    }

    let Some(path) = uploaded_root else {
        return Err(response::err_response(AppError::BadRequest(
            "no files were uploaded".to_string(),
        )));
    };

    let metadata = tokio::fs::metadata(&path).await.ok();
    let exists = metadata.is_some();
    let is_dir = metadata.as_ref().is_some_and(|meta| meta.is_dir());
    let has_skill_md = if is_dir {
        tokio::fs::metadata(path.join("SKILL.md")).await.is_ok()
    } else {
        false
    };

    Ok(response::ok(SkillUploadResult {
        path: path.to_string_lossy().to_string(),
        exists,
        is_dir,
        has_skill_md,
        uploaded_files,
    }))
}

fn sanitize_upload_relative_path(input: &str) -> Result<PathBuf, AppError> {
    use std::path::Component;

    let candidate = PathBuf::from(input.replace('\\', "/"));
    if candidate.is_absolute() {
        return Err(AppError::BadRequest("absolute file paths are not allowed".to_string()));
    }

    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::BadRequest("parent traversal is not allowed in uploaded paths".to_string()));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(AppError::BadRequest("uploaded file path cannot be empty".to_string()));
    }

    Ok(normalized)
}

#[utoipa::path(
    get,
    path = "/api/v2/admin/skills/events",
    params(("after" = Option<u64>, Query, description = "Return events with seq greater than this value")),
    responses((status = 200, description = "Recent skill tool events"))
)]
pub async fn get_skill_events(
    State(state): State<AppState>,
    Query(query): Query<SkillEventsQuery>,
) -> ApiResult<Value> {
    let events = state.skills.list_tool_events(query.after).await;
    let next_after = events
        .last()
        .map(|event| event.seq)
        .unwrap_or(query.after.unwrap_or(0));
    Ok(response::ok(json!({
        "events": events,
        "nextAfter": next_after
    })))
}

#[utoipa::path(
    get,
    path = "/api/v2/admin/skills/plans",
    responses((status = 200, description = "Active plans list", body = Vec<ActivePlanSummary>))
)]
pub async fn get_active_plans(State(state): State<AppState>) -> ApiResult<Vec<ActivePlanSummary>> {
    let plans = state.skills.list_active_plans().await;
    Ok(response::ok(plans))
}

#[utoipa::path(
    delete,
    path = "/api/v2/admin/skills/plans/{planning_id}",
    params(("planning_id" = String, Path, description = "Active planning id")),
    responses(
        (status = 200, description = "Plan removed"),
        (status = 404, description = "Planning id not found")
    )
)]
pub async fn delete_active_plan(
    State(state): State<AppState>,
    Path(planning_id): Path<String>,
) -> ApiResult<Value> {
    let removed = state.skills.delete_plan(&planning_id).await;
    if !removed {
        return Err(response::err_response(AppError::NotFound(format!(
            "planning id not found: {planning_id}"
        ))));
    }
    Ok(response::ok(
        json!({ "planningId": planning_id, "removed": true }),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v2/admin/skills/confirmations",
    responses((status = 200, description = "Pending skill confirmations"))
)]
pub async fn get_pending_skill_confirmations(
    State(state): State<AppState>,
) -> ApiResult<Vec<SkillConfirmation>> {
    let list = state.skills.list_pending_confirmations().await;
    Ok(response::ok(list))
}

#[utoipa::path(
    post,
    path = "/api/v2/admin/skills/confirmations/{confirmation_id}/approve",
    params(("confirmation_id" = String, Path, description = "Confirmation id")),
    responses((status = 200, description = "Approved confirmation"))
)]
pub async fn approve_skill_confirmation(
    State(state): State<AppState>,
    Path(confirmation_id): Path<String>,
) -> ApiResult<SkillConfirmation> {
    let updated = state
        .skills
        .approve_confirmation(&confirmation_id)
        .await
        .map_err(response::err_response)?;
    Ok(response::ok(updated))
}

#[utoipa::path(
    post,
    path = "/api/v2/admin/skills/confirmations/{confirmation_id}/reject",
    params(("confirmation_id" = String, Path, description = "Confirmation id")),
    responses((status = 200, description = "Rejected confirmation"))
)]
pub async fn reject_skill_confirmation(
    State(state): State<AppState>,
    Path(confirmation_id): Path<String>,
) -> ApiResult<SkillConfirmation> {
    let updated = state
        .skills
        .reject_confirmation(&confirmation_id)
        .await
        .map_err(response::err_response)?;
    Ok(response::ok(updated))
}

// ── AI Adapter 会话管理 API ──

#[utoipa::path(
    get,
    path = "/api/v2/admin/ai-sessions",
    responses((status = 200, description = "AI adapter sessions"))
)]
pub async fn get_ai_sessions(State(state): State<AppState>) -> ApiResult<Vec<AiSession>> {
    let sessions = state.ai_sessions.list_sessions().await;
    Ok(response::ok(sessions))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RenameSessionBody {
    pub name: String,
}

#[utoipa::path(
    post,
    path = "/api/v2/admin/ai-sessions/{session_id}/rename",
    request_body = RenameSessionBody,
    params(("session_id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Session renamed"))
)]
pub async fn rename_ai_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<RenameSessionBody>,
) -> ApiResult<AiSession> {
    let updated = state
        .ai_sessions
        .rename_session(&session_id, &body.name)
        .await
        .map_err(|e| response::err_response(AppError::BadRequest(e)))?;
    Ok(response::ok(updated))
}

#[utoipa::path(
    delete,
    path = "/api/v2/admin/ai-sessions/{session_id}",
    params(("session_id" = String, Path, description = "Session ID")),
    responses((status = 200, description = "Session deleted"))
)]
pub async fn delete_ai_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Value> {
    let removed = state.ai_sessions.remove_session(&session_id).await;
    Ok(response::ok(
        json!({"sessionId": session_id, "removed": removed}),
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ToggleToolBody {
    pub enabled: bool,
}

#[utoipa::path(
    put,
    path = "/api/v2/admin/ai-sessions/{session_id}/tools/{tool_name}",
    request_body = ToggleToolBody,
    params(
        ("session_id" = String, Path, description = "Session ID"),
        ("tool_name" = String, Path, description = "Tool name")
    ),
    responses((status = 200, description = "Tool toggled"))
)]
pub async fn toggle_ai_session_tool(
    State(state): State<AppState>,
    Path((session_id, tool_name)): Path<(String, String)>,
    Json(body): Json<ToggleToolBody>,
) -> ApiResult<AiToolDef> {
    let updated = state
        .ai_sessions
        .toggle_tool(&session_id, &tool_name, body.enabled)
        .await
        .map_err(|e| response::err_response(AppError::BadRequest(e)))?;
    Ok(response::ok(updated))
}
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateSystemPromptBody {
    pub text: Option<String>,
}

pub async fn update_ai_session_system_prompt(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<UpdateSystemPromptBody>,
) -> ApiResult<Value> {
    state
        .ai_sessions
        .update_system_prompt(&session_id, body.text.clone())
        .await
        .map_err(|e| response::err_response(AppError::BadRequest(e)))?;
    Ok(response::ok(
        json!({ "sessionId": session_id, "systemPromptOverride": body.text }),
    ))
}

pub async fn toggle_ai_session_system_prompt_tool(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<ToggleToolBody>,
) -> ApiResult<Value> {
    let enabled = state
        .ai_sessions
        .toggle_system_prompt_tool(&session_id, body.enabled)
        .await
        .map_err(|e| response::err_response(AppError::BadRequest(e)))?;
    Ok(response::ok(
        json!({ "sessionId": session_id, "systemPromptToolEnabled": enabled }),
    ))
}

pub async fn toggle_ai_session_tool_ping(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<ToggleToolBody>,
) -> ApiResult<Value> {
    let enabled = state
        .ai_sessions
        .toggle_tool_ping(&session_id, body.enabled)
        .await
        .map_err(|e| response::err_response(AppError::BadRequest(e)))?;
    Ok(response::ok(
        json!({ "sessionId": session_id, "toolPingEnabled": enabled }),
    ))
}

fn gateway_base_url(listen: &str) -> Result<String, AppError> {
    let addr: SocketAddr = listen
        .parse()
        .map_err(|_| AppError::Validation(format!("invalid listen address: {listen}")))?;

    let host = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };

    Ok(format!("http://{host}:{}", addr.port()))
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSystemInfo {
    pub os: String,
    pub arch: String,
    pub family: String,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRuntimeAvailability {
    pub installed: bool,
    pub version: Option<String>,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTerminalEncodingStatus {
    pub active_code_page: Option<u32>,
    pub utf8_forced: bool,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRuntimeSummary {
    pub system: RemoteSystemInfo,
    pub python: RemoteRuntimeAvailability,
    pub node: RemoteRuntimeAvailability,
    pub uv: RemoteRuntimeAvailability,
    pub terminal: RemoteTerminalEncodingStatus,
    pub config_path: String,
}

fn run_version_probe_remote(executable: &str, args: &[&str], extract_version: fn(&str) -> Option<String>) -> Option<String> {
    let mut command = Command::new(executable);
    command.args(args);
    
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let output = command.output().ok()?;
    for source in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(source);
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if let Some(version) = extract_version(line) {
                return Some(version);
            }
        }
    }
    None
}

fn extract_python_version_remote(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let version = trimmed.strip_prefix("Python ")?;
    version
        .chars()
        .next()
        .filter(|ch| ch.is_ascii_digit())
        .map(|_| trimmed.to_string())
}

fn extract_node_version_remote(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let version = trimmed.strip_prefix('v')?;
    version
        .chars()
        .next()
        .filter(|ch| ch.is_ascii_digit())
        .map(|_| trimmed.to_string())
}

fn extract_uv_version_remote(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let version = trimmed.strip_prefix("uv ")?;
    version
        .chars()
        .next()
        .filter(|ch| ch.is_ascii_digit())
        .map(|_| trimmed.to_string())
}

#[utoipa::path(
    get,
    path = "/api/v2/admin/runtimes",
    responses((status = 200, description = "Get server runtime environments", body = RemoteRuntimeSummary))
)]
pub async fn get_server_runtimes(State(state): State<AppState>) -> ApiResult<RemoteRuntimeSummary> {
    let system = RemoteSystemInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        family: std::env::consts::FAMILY.to_string(),
    };

    let python_version = if cfg!(target_os = "windows") {
        run_version_probe_remote("py", &["-3", "--version"], extract_python_version_remote)
            .or_else(|| run_version_probe_remote("python", &["--version"], extract_python_version_remote))
            .or_else(|| run_version_probe_remote("python3", &["--version"], extract_python_version_remote))
            .or_else(|| run_version_probe_remote("py", &["--version"], extract_python_version_remote))
    } else {
        run_version_probe_remote("python3", &["--version"], extract_python_version_remote)
            .or_else(|| run_version_probe_remote("python", &["--version"], extract_python_version_remote))
    };
    let python = RemoteRuntimeAvailability {
        installed: python_version.is_some(),
        version: python_version,
    };

    let node_version = run_version_probe_remote("node", &["--version"], extract_node_version_remote);
    let node = RemoteRuntimeAvailability {
        installed: node_version.is_some(),
        version: node_version,
    };

    let uv_version = run_version_probe_remote("uv", &["--version"], extract_uv_version_remote);
    let uv = RemoteRuntimeAvailability {
        installed: uv_version.is_some(),
        version: uv_version,
    };

    #[allow(unused_mut)]
    let mut active_code_page = None;
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Console::GetConsoleOutputCP;
        let cp = unsafe { GetConsoleOutputCP() };
        if cp != 0 {
            active_code_page = Some(cp);
        }
    }
    
    let terminal = RemoteTerminalEncodingStatus {
        active_code_page,
        utf8_forced: std::env::var("MCP_FORCE_UTF8").is_ok(),
    };

    let config_path = state.config_service.get_path().to_string_lossy().to_string();

    Ok(response::ok(RemoteRuntimeSummary {
        system,
        python,
        node,
        uv,
        terminal,
        config_path,
    }))
}
