use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, Notify, RwLock};
use uuid::Uuid;

/// Max concurrent pending calls per session
const MAX_PENDING_CALLS: usize = 16;

/// A pair of notify + result mutex used for resolving pending/inflight tool calls.
type NotifyResultPair = (Arc<Notify>, Arc<Mutex<Option<PendingToolResult>>>);

/// AI adapter session protocol format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AiProtocol {
    /// OpenAI Chat Completions (/v1/chat/completions)
    OpenaiChat,
    /// OpenAI Responses API (/v1/responses)
    OpenaiResponses,
    /// Anthropic Messages (/v1/messages)
    Anthropic,
}

impl AiProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            AiProtocol::OpenaiChat => "openai-chat",
            AiProtocol::OpenaiResponses => "openai-responses",
            AiProtocol::Anthropic => "anthropic",
        }
    }
}

/// Public session view returned to callers / UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSession {
    pub id: String,
    /// MCP server name in __name__ format
    pub name: String,
    /// Human-readable display name
    pub display_name: String,
    /// Source identifier from User-Agent
    pub source: String,
    /// Protocol used by this session
    pub protocol: AiProtocol,
    pub system_prompt: String,
    pub tools: Vec<AiToolDef>,
    pub connected_at: DateTime<Utc>,
    pub tool_count: usize,
    pub has_system_prompt: bool,

    /// Whether the system_prompt tool is enabled for this session
    pub system_prompt_tool_enabled: bool,
    /// Whether heartbeat should use a synthetic tool call for this session.
    pub tool_ping_enabled: bool,
    /// User-overridden system prompt text (None = use original)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<String>,
    pub has_pending_call: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// Whether this tool is exposed to MCP clients. Defaults to true.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// A pending tool call request: MCP client called a tool, waiting to forward to AI session
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub created_at: DateTime<Utc>,
    pub notify: Arc<Notify>,
    pub result: Arc<Mutex<Option<PendingToolResult>>>,
}

#[derive(Debug, Clone)]
pub struct PendingToolResult {
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug)]
struct SessionState {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub source: String,
    pub protocol: AiProtocol,
    pub system_prompt: String,
    pub system_prompt_tool_enabled: bool,
    pub tool_ping_enabled: bool,
    pub system_prompt_override: Option<String>,
    pub tools: Vec<AiToolDef>,
    pub connected_at: DateTime<Utc>,
    /// Pending tool calls queue
    pub pending_calls: VecDeque<PendingToolCall>,
    /// Inflight calls already dispatched to AI, awaiting result
    pub inflight_calls: HashMap<String, PendingToolCall>,
    /// Notify for waking AI-side waiters when queue becomes non-empty
    pub wake_notify: Arc<Notify>,
}

#[derive(Clone)]
pub struct AiSessionManager {
    sessions: Arc<RwLock<HashMap<String, SessionState>>>,
    /// Index by MCP server name (__name__ format) -> session_id
    name_index: Arc<RwLock<HashMap<String, String>>>,
    /// Monotonically increasing session counter for naming
    counter: Arc<tokio::sync::Mutex<u32>>,
}

impl Default for AiSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AiSessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            name_index: Arc::new(RwLock::new(HashMap::new())),
            counter: Arc::new(tokio::sync::Mutex::new(0)),
        }
    }

    /// Generate next session display name and MCP server name (always increments)
    async fn next_names(&self) -> (String, String) {
        let mut counter = self.counter.lock().await;
        *counter += 1;
        let display = format!("session-{}", *counter);
        let mcp_name = format!("__{}__", display);
        (display, mcp_name)
    }

    /// Always create a brand new session. No reuse.
    pub async fn create_session(
        &self,
        protocol: AiProtocol,
        system_prompt: String,
        tools: Vec<AiToolDef>,
        source: String,
    ) -> AiSession {
        let id = Uuid::new_v4().to_string();
        let (display_name, mcp_name) = self.next_names().await;
        let now = Utc::now();

        let state = SessionState {
            id: id.clone(),
            name: mcp_name.clone(),
            display_name: display_name.clone(),
            source: source.clone(),
            protocol,
            system_prompt: system_prompt.clone(),
            system_prompt_tool_enabled: true,
            tool_ping_enabled: true,
            system_prompt_override: None,
            tools: tools.clone(),
            connected_at: now,
            pending_calls: VecDeque::new(),
            inflight_calls: HashMap::new(),
            wake_notify: Arc::new(Notify::new()),
        };

        self.sessions.write().await.insert(id.clone(), state);
        self.name_index
            .write()
            .await
            .insert(mcp_name.clone(), id.clone());

        AiSession {
            id,
            name: mcp_name,
            display_name,
            source,
            protocol,
            system_prompt: system_prompt.clone(),
            system_prompt_tool_enabled: true,
            tool_ping_enabled: true,
            system_prompt_override: None,
            tool_count: tools.len(),
            tools,
            connected_at: now,
            has_system_prompt: !system_prompt.is_empty(),
            has_pending_call: false,
        }
    }

    /// Wait for a pending tool call on this session.
    /// Returns None if session no longer exists.
    pub async fn wait_for_pending_call(&self, session_id: &str) -> Option<PendingToolCall> {
        let wake = {
            let sessions = self.sessions.read().await;
            match sessions.get(session_id) {
                Some(s) => s.wake_notify.clone(),
                None => return None,
            }
        };

        loop {
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            {
                let mut sessions = self.sessions.write().await;
                if let Some(state) = sessions.get_mut(session_id) {
                    if let Some(call) = state.pending_calls.pop_front() {
                        state
                            .inflight_calls
                            .insert(call.call_id.clone(), call.clone());
                        return Some(call);
                    }
                } else {
                    return None;
                }
            }

            notified.await;
        }
    }

    fn state_to_session(&self, s: &SessionState) -> AiSession {
        AiSession {
            id: s.id.clone(),
            name: s.name.clone(),
            display_name: s.display_name.clone(),
            source: s.source.clone(),
            protocol: s.protocol,
            system_prompt: s.system_prompt.clone(),
            tools: s.tools.clone(),
            tool_count: s.tools.len(),
            connected_at: s.connected_at,
            has_system_prompt: !s.system_prompt.is_empty(),
            system_prompt_tool_enabled: s.system_prompt_tool_enabled,
            tool_ping_enabled: s.tool_ping_enabled,
            system_prompt_override: s.system_prompt_override.clone(),
            has_pending_call: !s.pending_calls.is_empty() || !s.inflight_calls.is_empty(),
        }
    }

    pub async fn get_session(&self, session_id: &str) -> Option<AiSession> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| self.state_to_session(s))
    }

    pub async fn get_session_by_name(&self, mcp_name: &str) -> Option<AiSession> {
        let name_index = self.name_index.read().await;
        let session_id = name_index.get(mcp_name)?;
        self.get_session(session_id).await
    }

    pub async fn list_sessions(&self) -> Vec<AiSession> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .map(|s| self.state_to_session(s))
            .collect()
    }

    /// Rename session (update display_name and MCP server name)
    pub async fn rename_session(
        &self,
        session_id: &str,
        new_display_name: &str,
    ) -> Result<AiSession, String> {
        let new_mcp_name = format!("__{}__", new_display_name);

        {
            let name_index = self.name_index.read().await;
            if let Some(existing_id) = name_index.get(&new_mcp_name) {
                if existing_id != session_id {
                    return Err(format!(
                        "Name '{}' is already used by another session",
                        new_display_name
                    ));
                }
            }
        }

        let mut sessions = self.sessions.write().await;
        let state = sessions
            .get_mut(session_id)
            .ok_or_else(|| "Session not found".to_string())?;

        let old_mcp_name = state.name.clone();
        state.display_name = new_display_name.to_string();
        state.name = new_mcp_name.clone();

        let mut name_index = self.name_index.write().await;
        name_index.remove(&old_mcp_name);
        name_index.insert(new_mcp_name, session_id.to_string());

        Ok(self.state_to_session(state))
    }

    pub async fn remove_session(&self, session_id: &str) -> bool {
        let mut sessions = self.sessions.write().await;
        if let Some(state) = sessions.remove(session_id) {
            self.name_index.write().await.remove(&state.name);
            true
        } else {
            false
        }
    }

    /// Enqueue a pending tool call and wake waiting AI-side
    pub async fn set_pending_tool_call(
        &self,
        session_id: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<PendingToolCall, String> {
        let (pending, wake) = {
            let mut sessions = self.sessions.write().await;
            let state = sessions
                .get_mut(session_id)
                .ok_or_else(|| "Session not found".to_string())?;

            if state.pending_calls.len() + state.inflight_calls.len() >= MAX_PENDING_CALLS {
                return Err("too many pending tool calls".to_string());
            }

            let call_id = format!("{}:{}", state.id, Uuid::new_v4());
            let pending = PendingToolCall {
                call_id,
                tool_name: tool_name.to_string(),
                arguments,
                created_at: Utc::now(),
                notify: Arc::new(Notify::new()),
                result: Arc::new(Mutex::new(None)),
            };

            state.pending_calls.push_back(pending.clone());
            let wake = state.wake_notify.clone();
            (pending, wake)
        };

        wake.notify_one();
        Ok(pending)
    }

    /// Submit tool call result and notify MCP side
    pub async fn resolve_tool_call(
        &self,
        session_id: &str,
        call_id: &str,
        result: PendingToolResult,
    ) -> Result<bool, String> {
        let targets: Vec<NotifyResultPair> = {
            let sessions = self.sessions.read().await;
            let state = sessions
                .get(session_id)
                .ok_or_else(|| "Session not found".to_string())?;

            let mut found = Vec::new();
            for p in state.pending_calls.iter().filter(|p| p.call_id == call_id) {
                found.push((p.notify.clone(), p.result.clone()));
            }
            if let Some(p) = state.inflight_calls.get(call_id) {
                found.push((p.notify.clone(), p.result.clone()));
            }
            found
        };

        if targets.is_empty() {
            return Ok(false);
        }

        for (notify, result_arc) in targets {
            let mut guard = result_arc.lock().await;
            *guard = Some(result.clone());
            notify.notify_one();
        }

        {
            let mut sessions = self.sessions.write().await;
            if let Some(state) = sessions.get_mut(session_id) {
                state.pending_calls.retain(|p| p.call_id != call_id);
                state.inflight_calls.remove(call_id);
            }
        }

        Ok(true)
    }

    /// Toggle a tool enabled/disabled for a session. Returns the updated tool or error.
    pub async fn toggle_tool(
        &self,
        session_id: &str,
        tool_name: &str,
        enabled: bool,
    ) -> Result<AiToolDef, String> {
        let mut sessions = self.sessions.write().await;
        let state = sessions
            .get_mut(session_id)
            .ok_or_else(|| "Session not found".to_string())?;

        let tool = state
            .tools
            .iter_mut()
            .find(|t| t.name == tool_name)
            .ok_or_else(|| format!("Tool not found: {tool_name}"))?;

        tool.enabled = enabled;
        Ok(tool.clone())
    }

    /// Toggle the system_prompt tool for a session
    pub async fn toggle_system_prompt_tool(
        &self,
        session_id: &str,
        enabled: bool,
    ) -> Result<bool, String> {
        let mut sessions = self.sessions.write().await;
        let state = sessions
            .get_mut(session_id)
            .ok_or_else(|| "Session not found".to_string())?;
        state.system_prompt_tool_enabled = enabled;
        Ok(enabled)
    }

    /// Toggle synthetic tool-call heartbeat for a session.
    pub async fn toggle_tool_ping(&self, session_id: &str, enabled: bool) -> Result<bool, String> {
        let mut sessions = self.sessions.write().await;
        let state = sessions
            .get_mut(session_id)
            .ok_or_else(|| "Session not found".to_string())?;
        state.tool_ping_enabled = enabled;
        Ok(enabled)
    }

    pub async fn tool_ping_enabled(&self, session_id: &str) -> Option<bool> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|state| state.tool_ping_enabled)
    }

    /// Update the system prompt override for a session
    pub async fn update_system_prompt(
        &self,
        session_id: &str,
        text: Option<String>,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.write().await;
        let state = sessions
            .get_mut(session_id)
            .ok_or_else(|| "Session not found".to_string())?;
        state.system_prompt_override = text;
        Ok(())
    }

    /// Get the effective system prompt text for a session (override or original)
    pub async fn get_effective_system_prompt(&self, session_id: &str) -> Option<(bool, String)> {
        let sessions = self.sessions.read().await;
        let state = sessions.get(session_id)?;
        let text = state
            .system_prompt_override
            .clone()
            .unwrap_or_else(|| state.system_prompt.clone());
        Some((state.system_prompt_tool_enabled, text))
    }
    /// Get effective system prompt by MCP server name
    pub async fn get_effective_system_prompt_by_name(
        &self,
        server_name: &str,
    ) -> Option<(bool, String)> {
        let name_index = self.name_index.read().await;
        let session_id = name_index.get(server_name)?.clone();
        drop(name_index);
        let sessions = self.sessions.read().await;
        let state = sessions.get(&session_id)?;
        let text = state
            .system_prompt_override
            .clone()
            .unwrap_or_else(|| state.system_prompt.clone());
        Some((state.system_prompt_tool_enabled, text))
    }
    pub async fn is_ai_adapter_server(&self, server_name: &str) -> bool {
        let name_index = self.name_index.read().await;
        name_index.contains_key(server_name)
    }

    pub async fn session_id_by_name(&self, server_name: &str) -> Option<String> {
        let name_index = self.name_index.read().await;
        name_index.get(server_name).cloned()
    }

    /// Find session ID by call_id. The call_id format is "{session_id}:{uuid}",
    /// so we can extract the session_id directly without searching.
    /// Falls back to scanning all sessions if the format doesn't match.
    pub async fn find_session_by_call_id(&self, call_id: &str) -> Option<String> {
        let sessions = self.sessions.read().await;
        // Try to extract session_id from call_id prefix
        if let Some(colon_pos) = call_id.find(':') {
            let candidate_id = &call_id[..colon_pos];
            if sessions.contains_key(candidate_id) {
                return Some(candidate_id.to_string());
            }
        }
        // Fallback: scan all sessions
        for (session_id, state) in sessions.iter() {
            if state.pending_calls.iter().any(|p| p.call_id == call_id) {
                return Some(session_id.clone());
            }
            if state.inflight_calls.contains_key(call_id) {
                return Some(session_id.clone());
            }
        }
        None
    }
}
