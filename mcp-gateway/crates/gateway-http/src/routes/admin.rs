use std::net::{IpAddr, SocketAddr};

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use gateway_core::{AppError, GatewayConfig, ServerConfig};
use serde::Deserialize;
use serde_json::{json, Value};

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

#[derive(Debug, Deserialize)]
pub struct SkillEventsQuery {
    after: Option<u64>,
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
