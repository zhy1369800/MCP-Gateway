use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use gateway_core::AiAdapterConfig;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::state::AppState;

use super::protocol::{
    ensure_input_schema_type_object, extract_anthropic_system_prompt, extract_anthropic_tools,
    extract_openai_system_prompt, extract_openai_tools, AnthropicRequest, OpenAiChatRequest,
    OpenAiRole,
};
use super::session::{AiProtocol, AiSessionManager, AiToolDef, PendingToolCall, PendingToolResult};

/// Heartbeat interval for the AI-side SSE stream. Must stay below 60 s so clients
/// (and intermediate proxies) don't drop the long-lived connection while idle.
const SSE_HEARTBEAT_INTERVAL_SECS: u64 = 50;
/// Model name declared in protocol responses
const RESPONSE_MODEL: &str = "mcp-gateway";

/// Generate a heartbeat token shaped like ping-<session_id>.
fn heartbeat_token(session_id: &str) -> String {
    format!("ping-{}", session_id)
}

// ── Helper functions ──

fn valid_keys(config: &AiAdapterConfig) -> Vec<&str> {
    config
        .api_keys
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .collect()
}

fn extract_api_key_from(headers: &HeaderMap, keys: &[&str]) -> String {
    if keys.is_empty() {
        return String::new();
    }

    let auth = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    for key in keys {
        let expected = format!("Bearer {}", key);
        if auth == expected {
            return key.to_string();
        }
    }
    String::new()
}

fn extract_session_id_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("X-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn extract_source(headers: &HeaderMap) -> String {
    headers
        .get("User-Agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
}

fn unauthorized_response() -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": {"message": "Invalid API key", "type": "authentication_error"}})),
    )
        .into_response()
}

fn json_to_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .map(|item| match item {
                Value::String(s) => s.clone(),
                Value::Object(map) => map
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| Value::Object(map.clone()).to_string()),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn arguments_to_json_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "{}".to_string(),
        other => other.to_string(),
    }
}

/// Check API key auth. Returns true if authorized (or no keys configured).
fn check_auth(headers: &HeaderMap, config: &AiAdapterConfig) -> bool {
    let keys = valid_keys(config);
    if keys.is_empty() {
        return true;
    }
    !extract_api_key_from(headers, &keys).is_empty()
}

/// Anthropic: x-api-key, no sk-ant prefix check
fn extract_anthropic_api_key_from(headers: &HeaderMap, keys: &[&str]) -> String {
    if keys.is_empty() {
        return String::new();
    }
    let api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    for key in keys {
        if api_key == *key {
            return key.to_string();
        }
    }
    String::new()
}

fn check_anthropic_auth(headers: &HeaderMap, config: &AiAdapterConfig) -> bool {
    let keys = valid_keys(config);
    if keys.is_empty() {
        return true;
    }
    if !extract_api_key_from(headers, &keys).is_empty() {
        return true;
    }
    !extract_anthropic_api_key_from(headers, &keys).is_empty()
}

// ── Routes ──

pub fn router(state: AppState, config: &AiAdapterConfig) -> Router {
    let base = config.base_path.trim_end_matches('/').to_string();
    // Register each AI route under both /v1/... and the legacy /v1/v1/... path
    // so that clients using either base-path convention can connect.
    let mut r = Router::new();
    macro_rules! ai_route {
        ($path:literal, $method:ident($handler:ident)) => {
            r = r
                .route(&format!(concat!("{}/v1", $path), base), $method($handler))
                .route(
                    &format!(concat!("{}/v1/v1", $path), base),
                    $method($handler),
                );
        };
    }
    ai_route!("/models", get(handle_models));
    ai_route!("/models/:model_id", get(handle_model_retrieve));
    ai_route!("/chat/completions", post(handle_openai_chat));
    ai_route!("/responses", post(handle_openai_responses));
    ai_route!("/messages", post(handle_anthropic_messages));
    ai_route!(
        "/messages/count_tokens",
        post(handle_anthropic_count_tokens)
    );
    r.route(&format!("{}/health", base), get(handle_ai_health))
        .with_state(state)
}

// ── GET endpoints ──

/// 已知模型清单：(id, display_name, owned_by)。
/// `/v1/models` 列表与 `/v1/models/{id}` 单查都从这里读取，
/// 这样不同协议风格的客户端拿到的内容保持一致。
const KNOWN_MODELS: &[(&str, &str, &str)] = &[
    (RESPONSE_MODEL, "MCP Gateway", "mcp-gateway"),
    ("claude-opus-4-7", "Claude Opus 4.7", "anthropic"),
    ("gpt-5.5", "GPT-5.5", "openai"),
];

/// Anthropic schema 要求 `created_at` 是 ISO-8601 字符串。
const ANTHROPIC_MODEL_CREATED_AT: &str = "2025-01-01T00:00:00Z";

/// Claude Code / Anthropic SDK 调用时一般会带 `anthropic-version`，
/// 部分场景下只用 `x-api-key` 而不是 `Authorization`。任一命中都按 Anthropic 风格响应。
fn is_anthropic_client(headers: &HeaderMap) -> bool {
    headers.contains_key("anthropic-version") || headers.contains_key("x-api-key")
}

fn openai_model_entry(id: &str, owned_by: &str) -> Value {
    json!({
        "id": id,
        "object": "model",
        "created": 1,
        "owned_by": owned_by,
    })
}

fn anthropic_model_entry(id: &str, display_name: &str) -> Value {
    json!({
        "type": "model",
        "id": id,
        "display_name": display_name,
        "created_at": ANTHROPIC_MODEL_CREATED_AT,
    })
}

/// `/v1/models*` 同时接受 `Authorization: Bearer ...` 与 `x-api-key`，
/// 避免 Claude Code 在做模型枚举/单查时被误判为未授权。
fn check_models_auth(headers: &HeaderMap, config: &AiAdapterConfig) -> bool {
    check_anthropic_auth(headers, config)
}

async fn handle_models(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let config = state.config_service.get_config().await;
    if !check_models_auth(&headers, &config.ai_adapter) {
        return unauthorized_response();
    }

    if is_anthropic_client(&headers) {
        let data: Vec<Value> = KNOWN_MODELS
            .iter()
            .map(|(id, display, _owner)| anthropic_model_entry(id, display))
            .collect();
        let first = data
            .first()
            .and_then(|v| v.get("id").cloned())
            .unwrap_or(Value::Null);
        let last = data
            .last()
            .and_then(|v| v.get("id").cloned())
            .unwrap_or(Value::Null);
        return Json(json!({
            "data": data,
            "has_more": false,
            "first_id": first,
            "last_id": last,
        }))
        .into_response();
    }

    let data: Vec<Value> = KNOWN_MODELS
        .iter()
        .map(|(id, _display, owner)| openai_model_entry(id, owner))
        .collect();
    Json(json!({
        "object": "list",
        "data": data,
    }))
    .into_response()
}

async fn handle_model_retrieve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> axum::response::Response {
    let config = state.config_service.get_config().await;
    if !check_models_auth(&headers, &config.ai_adapter) {
        return unauthorized_response();
    }

    let Some((id, display_name, owner)) = KNOWN_MODELS
        .iter()
        .find(|(known_id, _, _)| *known_id == model_id.as_str())
    else {
        let err_type = if is_anthropic_client(&headers) {
            "not_found_error"
        } else {
            "invalid_request_error"
        };
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": format!(
                        "model `{}` does not exist or you do not have access to it",
                        model_id
                    ),
                    "type": err_type,
                }
            })),
        )
            .into_response();
    };

    if is_anthropic_client(&headers) {
        Json(anthropic_model_entry(id, display_name)).into_response()
    } else {
        Json(openai_model_entry(id, owner)).into_response()
    }
}

async fn handle_ai_health(State(state): State<AppState>) -> impl IntoResponse {
    let sessions = state.ai_sessions.list_sessions().await;
    Json(json!({
        "status": "ok",
        "protocols": ["openai-chat", "openai-responses", "anthropic"],
        "active_sessions": sessions.len(),
        "sessions": sessions,
    }))
}

// ── OpenAI Chat Completions (SSE streaming) ──

fn chat_chunk(response_id: &str, choices: Value) -> Event {
    let chunk = json!({
        "id": response_id,
        "object": "chat.completion.chunk",
        "created": chrono::Utc::now().timestamp(),
        "model": RESPONSE_MODEL,
        "choices": choices,
    });
    Event::default().data(chunk.to_string())
}

fn yield_chat_tool_call_events(response_id: &str, tc: &PendingToolCall) -> Vec<Event> {
    let role_chunk = chat_chunk(
        response_id,
        json!([{ "index": 0, "delta": { "role": "assistant", "content": null }, "finish_reason": null }]),
    );

    let tool_chunk = chat_chunk(
        response_id,
        json!([{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": tc.call_id,
                    "type": "function",
                    "function": {
                        "name": tc.tool_name,
                        "arguments": arguments_to_json_string(&tc.arguments),
                    }
                }]
            },
            "finish_reason": null
        }]),
    );

    let finish_chunk = chat_chunk(
        response_id,
        json!([{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]),
    );

    let done = Event::default().data("[DONE]");
    vec![role_chunk, tool_chunk, finish_chunk, done]
}

async fn handle_openai_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let config = state.config_service.get_config().await;
    if !check_auth(&headers, &config.ai_adapter) {
        return unauthorized_response();
    }

    let session_id_header = extract_session_id_header(&headers);

    let request: OpenAiChatRequest = match serde_json::from_value(body) {
        Ok(req) => req,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": format!("Invalid request: {}", e), "type": "invalid_request_error"}})),
            ).into_response();
        }
    };

    // Tool result messages -> resolve on existing session
    let has_tool_results = request
        .messages
        .iter()
        .any(|m| matches!(m.role, OpenAiRole::Tool));
    if has_tool_results {
        let sid = if let Some(ref s) = session_id_header {
            s.clone()
        } else {
            // Fallback: find session by call_id from tool messages
            let call_id = request
                .messages
                .iter()
                .filter(|m| matches!(m.role, OpenAiRole::Tool))
                .filter_map(|m| m.tool_call_id.as_deref())
                .next()
                .unwrap_or("");
            match state.ai_sessions.find_session_by_call_id(call_id).await {
                Some(id) => id,
                None => {
                    return (StatusCode::BAD_REQUEST, Json(json!({"error": {"message": "Cannot find session for tool results", "type": "invalid_request_error"}}))).into_response();
                }
            }
        };
        return handle_openai_chat_tool_results(&state, &sid, &request).await;
    }

    // New request -> always create a new session
    let system_prompt = extract_openai_system_prompt(&request.messages);
    let mut tools = extract_openai_tools(&request.tools);
    let source = extract_source(&headers);

    // Apply disabled_tools from config snapshot taken above
    for tool in &mut tools {
        if config
            .disabled_tools
            .iter()
            .any(|dt| dt.matches(&tool.name))
        {
            tool.enabled = false;
        }
    }

    let session = state
        .ai_sessions
        .create_session(AiProtocol::OpenaiChat, system_prompt, tools, source)
        .await;

    chat_sse_response(state.ai_sessions.clone(), session.id)
}

async fn handle_openai_chat_tool_results(
    state: &AppState,
    session_id: &str,
    request: &OpenAiChatRequest,
) -> axum::response::Response {
    for msg in &request.messages {
        if !matches!(msg.role, OpenAiRole::Tool) {
            continue;
        }
        let Some(tool_call_id) = msg.tool_call_id.as_deref() else {
            continue;
        };
        let content = match &msg.content {
            Some(value) => json_to_text(value),
            None => String::new(),
        };
        let _ = state
            .ai_sessions
            .resolve_tool_call(
                session_id,
                tool_call_id,
                PendingToolResult {
                    content,
                    is_error: false,
                },
            )
            .await;
    }

    chat_sse_response(state.ai_sessions.clone(), session_id.to_string())
}

fn build_sse_response(
    session_id: &str,
    response_id_prefix: &str,
    id_len: usize,
) -> (String, String, mpsc::Sender<Event>, mpsc::Receiver<Event>) {
    let session_id_header = session_id.to_string();
    let response_id = format!(
        "{}{}",
        response_id_prefix,
        &Uuid::new_v4().to_string().replace('-', "")[..id_len]
    );
    let (tx, rx) = mpsc::channel::<Event>(16);
    (session_id_header, response_id, tx, rx)
}

fn sse_response_from_parts(
    session_id_header: String,
    rx: mpsc::Receiver<Event>,
) -> axum::response::Response {
    let stream = ReceiverStream::new(rx).map(Ok::<Event, Infallible>);
    let mut resp = Sse::new(stream).into_response();
    if let Ok(v) = axum::http::HeaderValue::from_str(&session_id_header) {
        resp.headers_mut().insert("X-Session-Id", v);
    }
    resp
}

fn chat_sse_response(
    ai_sessions: AiSessionManager,
    session_id: String,
) -> axum::response::Response {
    let (sid_hdr, response_id, tx, rx) = build_sse_response(&session_id, "chatcmpl-", 29);
    spawn_chat_pump(ai_sessions, session_id, response_id, tx);
    sse_response_from_parts(sid_hdr, rx)
}

fn spawn_chat_pump(
    ai_sessions: AiSessionManager,
    session_id: String,
    response_id: String,
    tx: mpsc::Sender<Event>,
) {
    tokio::spawn(async move {
        if run_chat_pump(&ai_sessions, &session_id, &response_id, &tx)
            .await
            .is_err()
        {
            ai_sessions.remove_session(&session_id).await;
        }
    });
}

async fn run_chat_pump(
    ai_sessions: &AiSessionManager,
    session_id: &str,
    response_id: &str,
    tx: &mpsc::Sender<Event>,
) -> Result<(), ()> {
    let dur = Duration::from_secs(SSE_HEARTBEAT_INTERVAL_SECS);
    let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + dur, dur);
    loop {
        tokio::select! {
            biased;
            call = ai_sessions.wait_for_pending_call(session_id) => {
                match call {
                    Some(tc) => {
                        for ev in yield_chat_tool_call_events(response_id, &tc) {
                            tx.send(ev).await.map_err(|_| ())?;
                        }
                        return Ok(());
                    }
                    None => return Ok(()),
                }
            }
            _ = heartbeat.tick() => {
                if ai_sessions.tool_ping_enabled(session_id).await.unwrap_or(true) {
                    for ev in chat_heartbeat_events(response_id, session_id) {
                        tx.send(ev).await.map_err(|_| ())?;
                    }
                    return Ok(());
                }
                tx.send(openai_sse_ping_event()).await.map_err(|_| ())?;
            }
        }
    }
}

fn openai_sse_ping_event() -> Event {
    Event::default().comment("keep-alive")
}

fn chat_heartbeat_events(response_id: &str, session_id: &str) -> Vec<Event> {
    let ping_name = heartbeat_token(session_id);
    let call_id = format!("{}:{}", session_id, Uuid::new_v4());

    let tool_chunk = chat_chunk(
        response_id,
        json!([{
            "index": 0,
            "delta": {
                "role": "assistant",
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": { "name": ping_name, "arguments": "{}" }
                }]
            },
            "finish_reason": null
        }]),
    );
    let finish = chat_chunk(
        response_id,
        json!([{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]),
    );
    let done = Event::default().data("[DONE]");
    vec![tool_chunk, finish, done]
}

// ── OpenAI Responses API (SSE streaming) ──

fn collect_responses_tool_outputs(input: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(items) = input.as_array() else {
        return out;
    };
    for item in items {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "function_call_output" {
            continue;
        }
        let call_id = item
            .get("call_id")
            .or_else(|| item.get("tool_call_id"))
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if call_id.is_empty() {
            continue;
        }
        let output = item.get("output").cloned().unwrap_or(Value::Null);
        out.push((call_id, json_to_text(&output)));
    }
    out
}

fn yield_responses_tool_call_events(
    response_id: &str,
    seq: &AtomicU64,
    tc: &PendingToolCall,
) -> Vec<Event> {
    let arguments = arguments_to_json_string(&tc.arguments);
    build_responses_tool_call_events(response_id, seq, &tc.tool_name, &arguments, &tc.call_id)
}

/// Build Responses tool-call SSE events. Shared by real tool calls and heartbeat.
fn build_responses_tool_call_events(
    response_id: &str,
    seq: &AtomicU64,
    tool_name: &str,
    arguments: &str,
    call_id: &str,
) -> Vec<Event> {
    let item_id = format!("fc_{}", &Uuid::new_v4().to_string().replace("-", "")[..24]);
    let next_seq = || seq.fetch_add(1, Ordering::SeqCst);

    let added = json!({
        "type": "response.output_item.added",
        "sequence_number": next_seq(),
        "output_index": 0,
        "item": {
            "type": "function_call",
            "id": item_id,
            "call_id": call_id,
            "name": tool_name,
            "arguments": "",
            "status": "in_progress",
        }
    });

    let delta = json!({
        "type": "response.function_call_arguments.delta",
        "sequence_number": next_seq(),
        "item_id": item_id,
        "output_index": 0,
        "delta": arguments,
    });

    let done = json!({
        "type": "response.function_call_arguments.done",
        "sequence_number": next_seq(),
        "item_id": item_id,
        "output_index": 0,
        "name": tool_name,
        "arguments": arguments,
    });

    let item_done = json!({
        "type": "response.output_item.done",
        "sequence_number": next_seq(),
        "output_index": 0,
        "item": {
            "type": "function_call",
            "id": item_id,
            "call_id": call_id,
            "name": tool_name,
            "arguments": arguments,
            "status": "completed",
        }
    });

    let completed = json!({
        "type": "response.completed",
        "sequence_number": next_seq(),
        "response": {
            "id": response_id,
            "object": "response",
            "created_at": chrono::Utc::now().timestamp(),
            "status": "completed",
            "model": RESPONSE_MODEL,
            "parallel_tool_calls": true,
            "tool_choice": "auto",
            "tools": [],
            "output": [{
                "type": "function_call",
                "id": item_id,
                "call_id": call_id,
                "name": tool_name,
                "arguments": arguments,
                "status": "completed",
            }]
        }
    });

    vec![
        Event::default()
            .event("response.output_item.added")
            .data(added.to_string()),
        Event::default()
            .event("response.function_call_arguments.delta")
            .data(delta.to_string()),
        Event::default()
            .event("response.function_call_arguments.done")
            .data(done.to_string()),
        Event::default()
            .event("response.output_item.done")
            .data(item_done.to_string()),
        Event::default()
            .event("response.completed")
            .data(completed.to_string()),
    ]
}

async fn handle_openai_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let config = state.config_service.get_config().await;
    if !check_auth(&headers, &config.ai_adapter) {
        return unauthorized_response();
    }

    let session_id_header = extract_session_id_header(&headers);

    // Check for tool result submission
    let input_value = body.get("input").cloned().unwrap_or(Value::Null);
    let tool_outputs = collect_responses_tool_outputs(&input_value);
    if !tool_outputs.is_empty() {
        let sid = if let Some(ref s) = session_id_header {
            s.clone()
        } else {
            let first_call_id = tool_outputs
                .first()
                .map(|(id, _)| id.as_str())
                .unwrap_or("");
            match state
                .ai_sessions
                .find_session_by_call_id(first_call_id)
                .await
            {
                Some(id) => id,
                None => {
                    return (StatusCode::BAD_REQUEST, Json(json!({"error": {"message": "Cannot find session for tool results", "type": "invalid_request_error"}}))).into_response();
                }
            }
        };
        return handle_openai_responses_tool_results(&state, &sid, tool_outputs).await;
    }

    // New request -> always create new session
    let system_prompt = body
        .get("instructions")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tools_value = body.get("tools").cloned().unwrap_or(Value::Array(vec![]));
    let mut tools: Vec<AiToolDef> = tools_value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let (name, description, parameters) = if let Some(func) = t.get("function") {
                        (
                            func.get("name").and_then(|v| v.as_str())?.to_string(),
                            func.get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            func.get("parameters")
                                .cloned()
                                .unwrap_or(Value::Object(serde_json::Map::new())),
                        )
                    } else {
                        (
                            t.get("name").and_then(|v| v.as_str())?.to_string(),
                            t.get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            t.get("parameters")
                                .cloned()
                                .unwrap_or(Value::Object(serde_json::Map::new())),
                        )
                    };
                    let input_schema = ensure_input_schema_type_object(parameters);
                    Some(AiToolDef {
                        name,
                        description,
                        input_schema,
                        enabled: true,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let source = extract_source(&headers);

    // Apply disabled_tools from config snapshot taken above
    for tool in &mut tools {
        if config
            .disabled_tools
            .iter()
            .any(|dt| dt.matches(&tool.name))
        {
            tool.enabled = false;
        }
    }

    let session = state
        .ai_sessions
        .create_session(AiProtocol::OpenaiResponses, system_prompt, tools, source)
        .await;

    responses_sse_response(state.ai_sessions.clone(), session.id)
}

async fn handle_openai_responses_tool_results(
    state: &AppState,
    session_id: &str,
    outputs: Vec<(String, String)>,
) -> axum::response::Response {
    for (call_id, content) in &outputs {
        let _ = state
            .ai_sessions
            .resolve_tool_call(
                session_id,
                call_id,
                PendingToolResult {
                    content: content.clone(),
                    is_error: false,
                },
            )
            .await;
    }

    responses_sse_response(state.ai_sessions.clone(), session_id.to_string())
}

fn responses_sse_response(
    ai_sessions: AiSessionManager,
    session_id: String,
) -> axum::response::Response {
    let (sid_hdr, response_id, tx, rx) = build_sse_response(&session_id, "resp_", 24);
    spawn_responses_pump(ai_sessions, session_id, response_id, tx);
    sse_response_from_parts(sid_hdr, rx)
}

fn spawn_responses_pump(
    ai_sessions: AiSessionManager,
    session_id: String,
    response_id: String,
    tx: mpsc::Sender<Event>,
) {
    tokio::spawn(async move {
        if run_responses_pump(&ai_sessions, &session_id, &response_id, &tx)
            .await
            .is_err()
        {
            ai_sessions.remove_session(&session_id).await;
        }
    });
}

async fn run_responses_pump(
    ai_sessions: &AiSessionManager,
    session_id: &str,
    response_id: &str,
    tx: &mpsc::Sender<Event>,
) -> Result<(), ()> {
    let seq = AtomicU64::new(0);

    // Send the protocol-required preamble events.
    let created_envelope = json!({
        "id": response_id,
        "object": "response",
        "created_at": chrono::Utc::now().timestamp(),
        "status": "in_progress",
        "model": RESPONSE_MODEL,
        "parallel_tool_calls": true,
        "tool_choice": "auto",
        "tools": [],
        "output": [],
    });
    let created = json!({
        "type": "response.created",
        "sequence_number": seq.fetch_add(1, Ordering::SeqCst),
        "response": created_envelope.clone(),
    });
    tx.send(
        Event::default()
            .event("response.created")
            .data(created.to_string()),
    )
    .await
    .map_err(|_| ())?;
    let in_progress = json!({
        "type": "response.in_progress",
        "sequence_number": seq.fetch_add(1, Ordering::SeqCst),
        "response": created_envelope,
    });
    tx.send(
        Event::default()
            .event("response.in_progress")
            .data(in_progress.to_string()),
    )
    .await
    .map_err(|_| ())?;

    let dur = Duration::from_secs(SSE_HEARTBEAT_INTERVAL_SECS);
    let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + dur, dur);
    loop {
        tokio::select! {
            biased;
            call = ai_sessions.wait_for_pending_call(session_id) => {
                match call {
                    Some(tc) => {
                        for ev in yield_responses_tool_call_events(response_id, &seq, &tc) {
                            tx.send(ev).await.map_err(|_| ())?;
                        }
                        return Ok(());
                    }
                    None => return Ok(()),
                }
            }
            _ = heartbeat.tick() => {
                if ai_sessions.tool_ping_enabled(session_id).await.unwrap_or(true) {
                    for ev in responses_heartbeat_events(response_id, session_id, &seq) {
                        tx.send(ev).await.map_err(|_| ())?;
                    }
                    return Ok(());
                }
                tx.send(openai_sse_ping_event()).await.map_err(|_| ())?;
            }
        }
    }
}

fn responses_heartbeat_events(response_id: &str, session_id: &str, seq: &AtomicU64) -> Vec<Event> {
    let ping_name = heartbeat_token(session_id);
    let call_id = format!("{}:{}", session_id, Uuid::new_v4());
    build_responses_tool_call_events(response_id, seq, &ping_name, "{}", &call_id)
}

// ── Anthropic Messages (SSE streaming) ──

fn count_text_for_tokens(value: &Value) -> usize {
    match value {
        Value::String(text) => text.chars().count(),
        Value::Array(items) => items.iter().map(count_text_for_tokens).sum(),
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                return text.chars().count();
            }
            map.values().map(count_text_for_tokens).sum()
        }
        Value::Null => 0,
        other => other.to_string().chars().count(),
    }
}

fn estimate_anthropic_input_tokens(body: &Value) -> u64 {
    let mut chars = 0usize;
    chars += body
        .get("system")
        .map(count_text_for_tokens)
        .unwrap_or_default();
    chars += body
        .get("messages")
        .map(count_text_for_tokens)
        .unwrap_or_default();
    chars += body
        .get("tools")
        .map(count_text_for_tokens)
        .unwrap_or_default();

    // Approximate enough for Claude Code preflight checks; this gateway does
    // not run a tokenizer or an upstream model.
    (chars / 4).max(1) as u64
}

async fn handle_anthropic_count_tokens(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let config = state.config_service.get_config().await;
    if !check_anthropic_auth(&headers, &config.ai_adapter) {
        return unauthorized_response();
    }

    Json(json!({
        "input_tokens": estimate_anthropic_input_tokens(&body),
    }))
    .into_response()
}

fn collect_anthropic_tool_results(request: &AnthropicRequest) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for msg in &request.messages {
        if msg.role != "user" {
            continue;
        }
        let Some(blocks) = msg.content.as_array() else {
            continue;
        };
        for block in blocks {
            let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
            if kind != "tool_result" {
                continue;
            }
            let id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let content = block.get("content").cloned().unwrap_or(Value::Null);
            out.push((id, json_to_text(&content)));
        }
    }
    out
}

async fn handle_anthropic_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let config = state.config_service.get_config().await;
    if !check_anthropic_auth(&headers, &config.ai_adapter) {
        return unauthorized_response();
    }

    let session_id_header = extract_session_id_header(&headers);

    let request: AnthropicRequest = match serde_json::from_value(body) {
        Ok(req) => req,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(json!({
                "error": {"message": format!("Invalid request: {}", e), "type": "invalid_request_error"}
            }))).into_response();
        }
    };

    let tool_results = collect_anthropic_tool_results(&request);
    if !tool_results.is_empty() {
        let sid = if let Some(ref s) = session_id_header {
            s.clone()
        } else {
            let first_call_id = tool_results
                .first()
                .map(|(id, _)| id.as_str())
                .unwrap_or("");
            match state.ai_sessions.find_session_by_call_id(first_call_id).await {
                Some(id) => id,
                None => return (StatusCode::BAD_REQUEST, Json(json!({"error": {"message": "Cannot find session for tool results", "type": "invalid_request_error"}}))).into_response(),
            }
        };
        return handle_anthropic_tool_results(&state, &sid, tool_results).await;
    }

    // New request -> always create new session
    let system_prompt = extract_anthropic_system_prompt(request.system.as_ref());
    let mut tools = extract_anthropic_tools(&request.tools);
    let source = extract_source(&headers);

    // Apply disabled_tools from config snapshot taken above
    for tool in &mut tools {
        if config
            .disabled_tools
            .iter()
            .any(|dt| dt.matches(&tool.name))
        {
            tool.enabled = false;
        }
    }

    let session = state
        .ai_sessions
        .create_session(AiProtocol::Anthropic, system_prompt, tools, source)
        .await;

    anthropic_sse_response(state.ai_sessions.clone(), session.id)
}

async fn handle_anthropic_tool_results(
    state: &AppState,
    session_id: &str,
    results: Vec<(String, String)>,
) -> axum::response::Response {
    for (call_id, content) in &results {
        let _ = state
            .ai_sessions
            .resolve_tool_call(
                session_id,
                call_id,
                PendingToolResult {
                    content: content.clone(),
                    is_error: false,
                },
            )
            .await;
    }

    anthropic_sse_response(state.ai_sessions.clone(), session_id.to_string())
}

fn yield_anthropic_tool_call_events(block_index: u64, tc: &PendingToolCall) -> Vec<Event> {
    let arguments_json = arguments_to_json_string(&tc.arguments);
    build_anthropic_tool_call_events(block_index, &tc.call_id, &tc.tool_name, &arguments_json)
}

/// Build Anthropic tool-call SSE events. Shared by real tool calls and heartbeat.
fn build_anthropic_tool_call_events(
    block_index: u64,
    call_id: &str,
    tool_name: &str,
    arguments_json: &str,
) -> Vec<Event> {
    let block_start = json!({
        "type": "content_block_start",
        "index": block_index,
        "content_block": {
            "type": "tool_use",
            "id": call_id,
            "name": tool_name,
            "input": {}
        }
    });

    let delta = json!({
        "type": "content_block_delta",
        "index": block_index,
        "delta": {
            "type": "input_json_delta",
            "partial_json": arguments_json
        }
    });

    let block_stop = json!({"type": "content_block_stop", "index": block_index});

    let msg_delta = json!({
        "type": "message_delta",
        "delta": {"stop_reason": "tool_use", "stop_sequence": null},
        "usage": {"output_tokens": 0}
    });

    let msg_stop = json!({"type": "message_stop"});

    vec![
        Event::default()
            .event("content_block_start")
            .data(block_start.to_string()),
        Event::default()
            .event("content_block_delta")
            .data(delta.to_string()),
        Event::default()
            .event("content_block_stop")
            .data(block_stop.to_string()),
        Event::default()
            .event("message_delta")
            .data(msg_delta.to_string()),
        Event::default()
            .event("message_stop")
            .data(msg_stop.to_string()),
    ]
}

fn anthropic_sse_response(
    ai_sessions: AiSessionManager,
    session_id: String,
) -> axum::response::Response {
    let (sid_hdr, response_id, tx, rx) = build_sse_response(&session_id, "msg_", 24);
    spawn_anthropic_pump(ai_sessions, session_id, response_id, tx);
    sse_response_from_parts(sid_hdr, rx)
}

fn spawn_anthropic_pump(
    ai_sessions: AiSessionManager,
    session_id: String,
    response_id: String,
    tx: mpsc::Sender<Event>,
) {
    tokio::spawn(async move {
        if run_anthropic_pump(&ai_sessions, &session_id, &response_id, &tx)
            .await
            .is_err()
        {
            ai_sessions.remove_session(&session_id).await;
        }
    });
}

async fn run_anthropic_pump(
    ai_sessions: &AiSessionManager,
    session_id: &str,
    response_id: &str,
    tx: &mpsc::Sender<Event>,
) -> Result<(), ()> {
    // Required preamble: open the assistant message.
    let start = json!({
        "type": "message_start",
        "message": {
            "id": response_id,
            "type": "message",
            "role": "assistant",
            "model": RESPONSE_MODEL,
            "content": [],
            "stop_reason": null,
            "stop_sequence": null,
            "usage": {"input_tokens": 0, "output_tokens": 0}
        }
    });
    tx.send(
        Event::default()
            .event("message_start")
            .data(start.to_string()),
    )
    .await
    .map_err(|_| ())?;

    let dur = Duration::from_secs(SSE_HEARTBEAT_INTERVAL_SECS);
    let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + dur, dur);
    loop {
        tokio::select! {
            biased;
            call = ai_sessions.wait_for_pending_call(session_id) => {
                match call {
                    Some(tc) => {
                        for ev in yield_anthropic_tool_call_events(0, &tc) {
                            tx.send(ev).await.map_err(|_| ())?;
                        }
                        return Ok(());
                    }
                    None => return Ok(()),
                }
            }
            _ = heartbeat.tick() => {
                if ai_sessions.tool_ping_enabled(session_id).await.unwrap_or(true) {
                    for ev in anthropic_heartbeat_events(session_id) {
                        tx.send(ev).await.map_err(|_| ())?;
                    }
                    return Ok(());
                }
                tx.send(anthropic_sse_ping_event()).await.map_err(|_| ())?;
            }
        }
    }
}

fn anthropic_sse_ping_event() -> Event {
    Event::default()
        .event("ping")
        .data(json!({"type": "ping"}).to_string())
}

fn anthropic_heartbeat_events(session_id: &str) -> Vec<Event> {
    let ping_name = heartbeat_token(session_id);
    let call_id = format!("{}:{}", session_id, Uuid::new_v4());
    build_anthropic_tool_call_events(0, &call_id, &ping_name, "{}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use chrono::Utc;
    use gateway_core::{save_config_atomic, ConfigService, GatewayConfig, ProcessManager};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::{SkillsService, SseHub};

    async fn test_router(api_keys: Vec<String>) -> Router {
        let dir = tempfile::tempdir().expect("temp config dir");
        let path = dir.path().join("config.json");
        let mut config = GatewayConfig::default();
        config.security.admin.token = "admin-test-token".to_string();
        config.ai_adapter.api_keys = api_keys;
        save_config_atomic(&path, &config).expect("save test config");

        let config_service = ConfigService::from_path(path)
            .await
            .expect("load test config");
        let state = AppState {
            config_service,
            process_manager: ProcessManager::new(),
            started_at: Utc::now(),
            sse_hub: SseHub::new(),
            skills: SkillsService::new(),
            ai_sessions: AiSessionManager::new(),
        };

        router(state, &config.ai_adapter)
    }

    async fn json_body(response: axum::response::Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    fn get(path: &str, auth_header: (&str, &str)) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(path)
            .header(auth_header.0, auth_header.1)
            .body(Body::empty())
            .expect("request")
    }

    fn post_json(path: &str, auth_header: (&str, &str), body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header(auth_header.0, auth_header.1)
            .body(Body::from(body.to_string()))
            .expect("request")
    }

    #[tokio::test]
    async fn double_v1_models_returns_same_list() {
        let app = test_router(vec!["test-key".to_string()]).await;

        let canonical = app
            .clone()
            .oneshot(get(
                "/api/v2/ai/v1/models",
                ("authorization", "Bearer test-key"),
            ))
            .await
            .expect("canonical response");
        assert_eq!(canonical.status(), StatusCode::OK);
        let canonical_body = json_body(canonical).await;

        let compat = app
            .oneshot(get(
                "/api/v2/ai/v1/v1/models",
                ("authorization", "Bearer test-key"),
            ))
            .await
            .expect("compat response");
        assert_eq!(compat.status(), StatusCode::OK);
        assert_eq!(json_body(compat).await, canonical_body);
    }

    #[tokio::test]
    async fn double_v1_model_retrieve_reuses_model_handler() {
        let app = test_router(vec!["test-key".to_string()]).await;

        let response = app
            .oneshot(get(
                "/api/v2/ai/v1/v1/models/claude-opus-4-7",
                ("x-api-key", "test-key"),
            ))
            .await
            .expect("model response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["id"], "claude-opus-4-7");
        assert_eq!(body["type"], "model");
    }

    #[tokio::test]
    async fn double_v1_count_tokens_accepts_bearer_and_x_api_key() {
        let app = test_router(vec!["test-key".to_string()]).await;
        let body = json!({
            "model": "claude-opus-4-7",
            "messages": [{"role": "user", "content": "hello"}],
        });

        for auth_header in [
            ("authorization", "Bearer test-key"),
            ("x-api-key", "test-key"),
        ] {
            let response = app
                .clone()
                .oneshot(post_json(
                    "/api/v2/ai/v1/v1/messages/count_tokens",
                    auth_header,
                    body.clone(),
                ))
                .await
                .expect("count_tokens response");
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                json_body(response).await["input_tokens"]
                    .as_u64()
                    .expect("input token count")
                    > 0
            );
        }
    }

    #[tokio::test]
    async fn double_v1_count_tokens_still_requires_key_when_configured() {
        let app = test_router(vec!["test-key".to_string()]).await;

        let request = Request::builder()
            .method("POST")
            .uri("/api/v2/ai/v1/v1/messages/count_tokens")
            .header("content-type", "application/json")
            .body(Body::from(json!({"messages": []}).to_string()))
            .expect("request");
        let response = app.oneshot(request).await.expect("response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn double_v1_messages_streams_anthropic_message_start() {
        let app = test_router(vec!["test-key".to_string()]).await;
        let request = Request::builder()
            .method("POST")
            .uri("/api/v2/ai/v1/v1/messages")
            .header("content-type", "application/json")
            .header("authorization", "Bearer test-key")
            .header("anthropic-version", "2023-06-01")
            .body(Body::from(
                json!({
                    "model": "claude-opus-4-7",
                    "max_tokens": 64,
                    "messages": [{"role": "user", "content": "hello"}],
                    "stream": true
                })
                .to_string(),
            ))
            .expect("request");

        let response = app.oneshot(request).await.expect("messages response");
        assert_eq!(response.status(), StatusCode::OK);

        let mut body = response.into_body();
        let frame = body
            .frame()
            .await
            .expect("first sse frame")
            .expect("valid sse frame");
        let data = frame.into_data().expect("sse data frame");
        let text = String::from_utf8(data.to_vec()).expect("utf8 sse");
        assert!(text.contains("message_start"));
    }
}
