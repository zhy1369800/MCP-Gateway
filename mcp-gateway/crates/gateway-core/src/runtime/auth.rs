use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::{Mutex, RwLock};

use crate::config::ServerConfig;
use crate::error::AppError;

use super::protocol_negotiation::NegotiatedStdioProtocol;

const AUTH_SCORE_THRESHOLD: i32 = 4;
const DEFAULT_AUTH_TIMEOUT_SECS: u64 = 120;
const DEFAULT_BROWSER_DEDUP_WINDOW_SECS: u64 = 8;
const MAX_SIGNAL_LINES: usize = 80;

pub type AuthBrowserOpener = Arc<dyn Fn(String) -> Result<(), String> + Send + Sync + 'static>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthSessionStatus {
    #[default]
    Idle,
    Starting,
    AuthPending,
    BrowserOpened,
    WaitingCallback,
    Authorized,
    Connected,
    AuthTimeout,
    AuthFailed,
    LaunchFailed,
    InitFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthSignalSource {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthSignalEvent {
    pub source: AuthSignalSource,
    pub line: String,
    pub score: i32,
    pub authorize_url: Option<String>,
    pub browser_opened: bool,
    pub waiting_for_authorization: bool,
    pub authorization_completed: bool,
    pub error_detected: bool,
    pub callback_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerAuthState {
    pub status: AuthSessionStatus,
    pub authorize_url: Option<String>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_updated_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub adapter_kind: Option<String>,
    pub browser_opened: bool,
    pub session_key: String,
    pub session_dir: Option<String>,
}

#[derive(Clone)]
pub struct AuthSessionStore {
    base_dir: PathBuf,
}

impl AuthSessionStore {
    pub fn new() -> Result<Self, AppError> {
        let base_dir = dirs::config_dir()
            .ok_or_else(|| AppError::Internal("Invalid auth session path".to_string()))?
            .join("mcp-gateway")
            .join("auth-sessions");
        Ok(Self { base_dir })
    }

    pub fn new_in(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn session_root(&self, session_key: &str) -> PathBuf {
        self.base_dir.join(session_key)
    }

    pub async fn load(&self, session_key: &str) -> Result<Option<ServerAuthState>, AppError> {
        let path = self.session_root(session_key).join("session.json");
        match fs::read_to_string(&path).await {
            Ok(content) => Ok(Some(serde_json::from_str(&content)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn save(&self, state: &ServerAuthState) -> Result<(), AppError> {
        let root = self.session_root(&state.session_key);
        fs::create_dir_all(&root).await?;
        let content = serde_json::to_string_pretty(state)?;
        fs::write(root.join("session.json"), content).await?;
        Ok(())
    }

    pub async fn clear(&self, session_key: &str) -> Result<(), AppError> {
        let root = self.session_root(session_key);
        match fs::remove_dir_all(root).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnownAdapterKind {
    McpRemote,
}

impl KnownAdapterKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::McpRemote => "mcp_remote",
        }
    }
}

#[derive(Debug, Clone)]
struct KnownAdapter {
    kind: KnownAdapterKind,
    resource_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedServerLaunch {
    pub server: ServerConfig,
    pub session_key: String,
    pub session_root: PathBuf,
    pub adapter_kind: Option<String>,
    pub preferred_protocol: Option<NegotiatedStdioProtocol>,
    pub auth_timeout: Duration,
}

#[derive(Clone)]
pub struct AuthOrchestrator {
    store: AuthSessionStore,
    browser_opener: Option<AuthBrowserOpener>,
    browser_opened_at: Arc<RwLock<HashMap<String, Instant>>>,
}

#[derive(Clone)]
pub(crate) struct RuntimeAuthState {
    orchestrator: AuthOrchestrator,
    prepared: PreparedServerLaunch,
    inner: Arc<Mutex<RuntimeAuthSnapshot>>,
}

#[derive(Debug, Clone)]
struct RuntimeAuthSnapshot {
    status: AuthSessionStatus,
    score: i32,
    authorize_url: Option<String>,
    browser_opened: bool,
    waiting_for_authorization: bool,
    authorization_completed: bool,
    auth_detected_at: Option<Instant>,
    last_success_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    recent_lines: VecDeque<String>,
}

impl AuthOrchestrator {
    pub fn new() -> Self {
        Self {
            store: AuthSessionStore::new()
                .unwrap_or_else(|_| AuthSessionStore::new_in(PathBuf::from(".mcp-gateway-auth"))),
            browser_opener: None,
            browser_opened_at: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_browser_opener(browser_opener: AuthBrowserOpener) -> Self {
        Self {
            browser_opener: Some(browser_opener),
            ..Self::new()
        }
    }

    pub fn with_store(store: AuthSessionStore) -> Self {
        Self {
            store,
            browser_opener: None,
            browser_opened_at: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) fn prepare_server(
        &self,
        server: &ServerConfig,
    ) -> Result<PreparedServerLaunch, AppError> {
        let adapter = detect_known_adapter(server);
        let session_key = session_key_for_server(server, adapter.as_ref());
        let session_root = self.store.session_root(&session_key);
        let mut managed_server = server.clone();

        ensure_generic_npm_cache(&mut managed_server, &session_root);

        let (preferred_protocol, adapter_kind, auth_timeout) = if let Some(adapter) = adapter {
            let timeout = ensure_adapter_defaults(&mut managed_server, &adapter, &session_root);
            (
                Some(NegotiatedStdioProtocol::JsonLines),
                Some(adapter.kind.as_str().to_string()),
                timeout,
            )
        } else {
            (None, None, Duration::from_secs(DEFAULT_AUTH_TIMEOUT_SECS))
        };

        Ok(PreparedServerLaunch {
            server: managed_server,
            session_key,
            session_root,
            adapter_kind,
            preferred_protocol,
            auth_timeout,
        })
    }

    pub async fn auth_state_for_server(
        &self,
        server: &ServerConfig,
    ) -> Result<ServerAuthState, AppError> {
        let prepared = self.prepare_server(server)?;
        self.load_or_default_state(&prepared).await
    }

    pub async fn clear_auth_state(
        &self,
        server: &ServerConfig,
    ) -> Result<ServerAuthState, AppError> {
        let prepared = self.prepare_server(server)?;
        self.store.clear(&prepared.session_key).await?;
        self.browser_opened_at
            .write()
            .await
            .remove(&prepared.session_key);
        Ok(self.default_state(&prepared))
    }

    pub(crate) async fn mark_launch_failed(
        &self,
        prepared: &PreparedServerLaunch,
        message: String,
    ) {
        let mut state = self.default_state(prepared);
        state.status = AuthSessionStatus::LaunchFailed;
        state.last_error = Some(message);
        state.last_updated_at = Some(Utc::now());
        let _ = self.store.save(&state).await;
    }

    async fn load_or_default_state(
        &self,
        prepared: &PreparedServerLaunch,
    ) -> Result<ServerAuthState, AppError> {
        Ok(self
            .store
            .load(&prepared.session_key)
            .await?
            .unwrap_or_else(|| self.default_state(prepared)))
    }

    fn default_state(&self, prepared: &PreparedServerLaunch) -> ServerAuthState {
        ServerAuthState {
            status: AuthSessionStatus::Idle,
            authorize_url: None,
            last_success_at: None,
            last_updated_at: None,
            last_error: None,
            adapter_kind: prepared.adapter_kind.clone(),
            browser_opened: false,
            session_key: prepared.session_key.clone(),
            session_dir: Some(prepared.session_root.display().to_string()),
        }
    }

    async fn save_state(&self, state: &ServerAuthState) {
        let _ = self.store.save(state).await;
    }

    async fn maybe_open_browser(
        &self,
        session_key: &str,
        authorize_url: &str,
    ) -> Result<bool, String> {
        let Some(opener) = &self.browser_opener else {
            return Ok(false);
        };

        {
            let guard = self.browser_opened_at.read().await;
            if let Some(last_opened) = guard.get(session_key) {
                if last_opened.elapsed() < Duration::from_secs(DEFAULT_BROWSER_DEDUP_WINDOW_SECS) {
                    return Ok(false);
                }
            }
        }

        (opener)(authorize_url.to_string())?;

        self.browser_opened_at
            .write()
            .await
            .insert(session_key.to_string(), Instant::now());
        Ok(true)
    }
}

impl Default for AuthOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeAuthState {
    pub(crate) async fn new(
        orchestrator: AuthOrchestrator,
        prepared: PreparedServerLaunch,
    ) -> Self {
        let mut snapshot = RuntimeAuthSnapshot {
            status: AuthSessionStatus::Starting,
            score: 0,
            authorize_url: None,
            browser_opened: false,
            waiting_for_authorization: false,
            authorization_completed: false,
            auth_detected_at: None,
            last_success_at: None,
            last_error: None,
            recent_lines: VecDeque::with_capacity(MAX_SIGNAL_LINES),
        };

        if let Ok(Some(existing)) = orchestrator.store.load(&prepared.session_key).await {
            snapshot.status = existing.status;
            snapshot.authorize_url = existing.authorize_url;
            snapshot.last_success_at = existing.last_success_at;
            snapshot.last_error = existing.last_error;
            snapshot.browser_opened = existing.browser_opened;
        }

        let this = Self {
            orchestrator,
            prepared,
            inner: Arc::new(Mutex::new(snapshot)),
        };
        this.persist_state().await;
        this
    }

    pub(crate) async fn handle_output_line(&self, source: AuthSignalSource, line: String) {
        let event = detect_auth_signal(&line, source);

        let mut maybe_open_url: Option<String> = None;
        let mut state = {
            let mut guard = self.inner.lock().await;
            if guard.recent_lines.len() == MAX_SIGNAL_LINES {
                let _ = guard.recent_lines.pop_front();
            }
            guard.recent_lines.push_back(line.clone());

            if event.score > 0 || event.authorize_url.is_some() {
                guard.score = guard.score.saturating_add(event.score);
            }
            if guard.auth_detected_at.is_none()
                && (guard.score >= AUTH_SCORE_THRESHOLD
                    || event.authorize_url.is_some()
                    || event.waiting_for_authorization
                    || event.browser_opened)
            {
                guard.auth_detected_at = Some(Instant::now());
            }
            if let Some(authorize_url) = event.authorize_url.clone() {
                if guard.authorize_url.is_none() {
                    guard.authorize_url = Some(authorize_url.clone());
                }
                if !event.browser_opened {
                    maybe_open_url = Some(authorize_url);
                }
            }

            if event.browser_opened {
                guard.browser_opened = true;
                guard.status = AuthSessionStatus::BrowserOpened;
            }
            if event.waiting_for_authorization {
                guard.waiting_for_authorization = true;
                if !guard.browser_opened {
                    guard.status = AuthSessionStatus::AuthPending;
                } else {
                    guard.status = AuthSessionStatus::WaitingCallback;
                }
            }
            if event.authorization_completed {
                guard.authorization_completed = true;
                guard.last_error = None;
                guard.status = AuthSessionStatus::Authorized;
            }
            if event.error_detected {
                guard.last_error = Some(event.line.clone());
                if matches!(
                    guard.status,
                    AuthSessionStatus::AuthPending
                        | AuthSessionStatus::BrowserOpened
                        | AuthSessionStatus::WaitingCallback
                ) {
                    guard.status = AuthSessionStatus::AuthFailed;
                }
            }
            if matches!(guard.status, AuthSessionStatus::Starting)
                && (guard.score >= AUTH_SCORE_THRESHOLD || guard.authorize_url.is_some())
            {
                guard.status = AuthSessionStatus::AuthPending;
            }

            self.to_server_auth_state_locked(&guard)
        };

        if let Some(authorize_url) = maybe_open_url {
            match self
                .orchestrator
                .maybe_open_browser(&self.prepared.session_key, &authorize_url)
                .await
            {
                Ok(opened) if opened => {
                    let mut guard = self.inner.lock().await;
                    guard.browser_opened = true;
                    if guard.waiting_for_authorization {
                        guard.status = AuthSessionStatus::WaitingCallback;
                    } else {
                        guard.status = AuthSessionStatus::BrowserOpened;
                    }
                    state = self.to_server_auth_state_locked(&guard);
                }
                Err(error) => {
                    let mut guard = self.inner.lock().await;
                    guard.last_error = Some(error);
                    if matches!(guard.status, AuthSessionStatus::Starting) {
                        guard.status = AuthSessionStatus::LaunchFailed;
                    }
                    state = self.to_server_auth_state_locked(&guard);
                }
                Ok(_) => {}
            }
        }

        self.orchestrator.save_state(&state).await;
    }

    pub(crate) async fn should_continue_waiting_for_auth(&self) -> bool {
        let guard = self.inner.lock().await;
        if !matches!(
            guard.status,
            AuthSessionStatus::AuthPending
                | AuthSessionStatus::BrowserOpened
                | AuthSessionStatus::WaitingCallback
                | AuthSessionStatus::Authorized
        ) {
            return false;
        }

        let Some(detected_at) = guard.auth_detected_at else {
            return false;
        };

        detected_at.elapsed() < self.prepared.auth_timeout
    }

    pub(crate) async fn mark_connected(&self) {
        let mut guard = self.inner.lock().await;
        guard.status = AuthSessionStatus::Connected;
        guard.last_success_at = Some(Utc::now());
        guard.last_error = None;
        let state = self.to_server_auth_state_locked(&guard);
        drop(guard);
        self.orchestrator.save_state(&state).await;
    }

    pub(crate) async fn mark_timeout(&self) {
        let mut guard = self.inner.lock().await;
        guard.status = if guard.auth_detected_at.is_some() {
            AuthSessionStatus::AuthTimeout
        } else {
            AuthSessionStatus::InitFailed
        };
        if guard.last_error.is_none() {
            guard.last_error = Some(
                "Timed out waiting for server response while authentication was incomplete"
                    .to_string(),
            );
        }
        let state = self.to_server_auth_state_locked(&guard);
        drop(guard);
        self.orchestrator.save_state(&state).await;
    }

    pub(crate) async fn mark_request_error(&self, error: &AppError) {
        let mut guard = self.inner.lock().await;
        if !matches!(guard.status, AuthSessionStatus::Connected) {
            guard.status = if guard.auth_detected_at.is_some() {
                AuthSessionStatus::AuthFailed
            } else {
                AuthSessionStatus::InitFailed
            };
        }
        guard.last_error = Some(error.to_string());
        let state = self.to_server_auth_state_locked(&guard);
        drop(guard);
        self.orchestrator.save_state(&state).await;
    }

    pub(crate) async fn stderr_snapshot(&self) -> String {
        let guard = self.inner.lock().await;
        let lines = guard.recent_lines.iter().cloned().collect::<Vec<_>>();
        let start = lines.len().saturating_sub(6);
        lines[start..].join(" | ")
    }

    pub(crate) async fn current_state(&self) -> ServerAuthState {
        let guard = self.inner.lock().await;
        self.to_server_auth_state_locked(&guard)
    }

    async fn persist_state(&self) {
        let state = self.current_state().await;
        self.orchestrator.save_state(&state).await;
    }

    fn to_server_auth_state_locked(&self, guard: &RuntimeAuthSnapshot) -> ServerAuthState {
        ServerAuthState {
            status: guard.status.clone(),
            authorize_url: guard.authorize_url.clone(),
            last_success_at: guard.last_success_at,
            last_updated_at: Some(Utc::now()),
            last_error: guard.last_error.clone(),
            adapter_kind: self.prepared.adapter_kind.clone(),
            browser_opened: guard.browser_opened,
            session_key: self.prepared.session_key.clone(),
            session_dir: Some(self.prepared.session_root.display().to_string()),
        }
    }
}

fn ensure_generic_npm_cache(server: &mut ServerConfig, session_root: &Path) {
    if !is_npx_like_command(&server.command) || env_contains_key(&server.env, "npm_config_cache") {
        return;
    }

    server.env.insert(
        "npm_config_cache".to_string(),
        session_root.join("npm-cache").display().to_string(),
    );
}

fn ensure_adapter_defaults(
    server: &mut ServerConfig,
    adapter: &KnownAdapter,
    session_root: &Path,
) -> Duration {
    match adapter.kind {
        KnownAdapterKind::McpRemote => {
            if !env_contains_key(&server.env, "MCP_REMOTE_CONFIG_DIR") {
                server.env.insert(
                    "MCP_REMOTE_CONFIG_DIR".to_string(),
                    session_root.join("mcp-remote").display().to_string(),
                );
            }
            if is_npx_like_command(&server.command)
                && !server.args.iter().any(|arg| arg == "-y" || arg == "--yes")
            {
                server.args.insert(0, "-y".to_string());
            }
            let timeout_secs = ensure_flag_with_value(&mut server.args, "--auth-timeout", "120")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_AUTH_TIMEOUT_SECS);
            Duration::from_secs(timeout_secs)
        }
    }
}

fn ensure_flag_with_value(
    args: &mut Vec<String>,
    flag: &str,
    default_value: &str,
) -> Option<String> {
    if let Some(index) = args.iter().position(|item| item == flag) {
        return args.get(index + 1).cloned();
    }

    args.push(flag.to_string());
    args.push(default_value.to_string());
    Some(default_value.to_string())
}

fn env_contains_key(env: &HashMap<String, String>, key: &str) -> bool {
    env.keys()
        .any(|existing| existing.eq_ignore_ascii_case(key))
}

fn detect_known_adapter(server: &ServerConfig) -> Option<KnownAdapter> {
    let command_name = normalized_command_name(&server.command);
    if command_name.eq_ignore_ascii_case("mcp-remote") {
        let remote_url = server
            .args
            .iter()
            .find(|arg| !arg.starts_with('-'))?
            .clone();
        let resource_id = build_mcp_remote_resource_id(&server.args, &remote_url);
        return Some(KnownAdapter {
            kind: KnownAdapterKind::McpRemote,
            resource_id,
        });
    }

    if !is_npx_like_command(&server.command) {
        return None;
    }

    let package_index = server.args.iter().position(|arg| {
        let normalized = normalize_package_spec(arg);
        normalized == "mcp-remote"
    })?;
    let remote_url = server
        .args
        .iter()
        .skip(package_index + 1)
        .find(|arg| !arg.starts_with('-'))?
        .clone();
    let resource_id = build_mcp_remote_resource_id(&server.args[package_index + 1..], &remote_url);
    Some(KnownAdapter {
        kind: KnownAdapterKind::McpRemote,
        resource_id,
    })
}

fn build_mcp_remote_resource_id(args: &[String], remote_url: &str) -> String {
    let mut resource = String::new();
    let mut headers = Vec::new();
    let mut index = 0_usize;
    while index < args.len() {
        match args[index].as_str() {
            "--resource" if index + 1 < args.len() => {
                resource = args[index + 1].clone();
                index += 2;
            }
            "--header" if index + 1 < args.len() => {
                headers.push(args[index + 1].clone());
                index += 2;
            }
            _ => {
                index += 1;
            }
        }
    }
    headers.sort();
    format!(
        "{remote_url}|resource={resource}|headers={}",
        headers.join("\u{001f}")
    )
}

fn session_key_for_server(server: &ServerConfig, adapter: Option<&KnownAdapter>) -> String {
    let mut env_items = server
        .env
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>();
    env_items.sort();

    let adapter_resource = adapter
        .map(|item| item.resource_id.clone())
        .unwrap_or_default();
    let fingerprint = format!(
        "{}|{}|{}|{}|{}",
        normalized_command_name(&server.command),
        server.args.join("\u{001f}"),
        server.cwd,
        env_items.join("\u{001e}"),
        adapter_resource
    );
    format!("{:016x}", fnv1a_64(fingerprint.as_bytes()))
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn normalized_command_name(command: &str) -> String {
    let path = Path::new(command);
    let raw = path
        .file_stem()
        .or_else(|| path.file_name())
        .and_then(|item| item.to_str())
        .unwrap_or(command);
    raw.to_ascii_lowercase()
}

fn normalize_package_spec(spec: &str) -> String {
    let raw = spec.trim();
    if raw.is_empty() {
        return String::new();
    }

    raw.split('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn is_npx_like_command(command: &str) -> bool {
    matches!(
        normalized_command_name(command).as_str(),
        "npx" | "npm" | "pnpm" | "pnpx"
    )
}

pub fn detect_auth_signal(line: &str, source: AuthSignalSource) -> AuthSignalEvent {
    let lower = line.to_ascii_lowercase();
    let authorize_url = extract_authorize_url(line, &lower);
    let browser_opened = contains_any(
        &lower,
        &[
            "browser opened",
            "opened automatically",
            "opening browser",
            "open browser",
        ],
    );
    let waiting_for_authorization = contains_any(
        &lower,
        &[
            "waiting for authorization",
            "waiting for auth code",
            "waiting for authentication",
            "wait-for-auth",
            "oauth callback",
            "callback server",
        ],
    );
    let authorization_completed = contains_any(
        &lower,
        &[
            "authentication completed",
            "using tokens from disk",
            "token saved",
            "proxy established successfully",
            "auth completed",
            "auth code received",
            "logged in successfully",
        ],
    );
    let error_detected = contains_any(
        &lower,
        &[
            "authentication error",
            "token exchange failed",
            "authorization denied",
            "access denied",
            "auth failed",
            "login failed",
        ],
    );
    let callback_port = extract_port_hint(line, &lower);

    let mut score = 0_i32;
    if authorize_url.is_some() {
        score += 4;
    }
    if contains_any(
        &lower,
        &[
            "oauth",
            "authorize",
            "authorization",
            "login",
            "consent",
            "callback",
        ],
    ) {
        score += 2;
    }
    if browser_opened {
        score += 2;
    }
    if waiting_for_authorization {
        score += 2;
    }
    if callback_port.is_some() {
        score += 1;
    }
    if authorization_completed || error_detected {
        score += 2;
    }

    AuthSignalEvent {
        source,
        line: line.to_string(),
        score,
        authorize_url,
        browser_opened,
        waiting_for_authorization,
        authorization_completed,
        error_detected,
        callback_port,
    }
}

fn contains_any(lower: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| lower.contains(pattern))
}

fn extract_authorize_url(line: &str, lower: &str) -> Option<String> {
    let mut urls = line
        .split_whitespace()
        .filter_map(|token| {
            let trimmed =
                token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | '(' | ')' | ',' | ';'));
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    urls.sort();
    urls.into_iter().find(|url| {
        let url_lower = url.to_ascii_lowercase();
        url_lower.contains("authorize")
            || url_lower.contains("oauth")
            || url_lower.contains("login")
            || url_lower.contains("consent")
            || lower.contains("please authorize")
            || lower.contains("open browser")
    })
}

fn extract_port_hint(line: &str, lower: &str) -> Option<u16> {
    if !(lower.contains("port") || lower.contains("callback")) {
        return None;
    }

    for token in line.split(|ch: char| !ch.is_ascii_digit()) {
        if token.len() < 2 || token.len() > 5 {
            continue;
        }
        if let Ok(port) = token.parse::<u16>() {
            if port > 0 {
                return Some(port);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_server() -> ServerConfig {
        ServerConfig {
            name: "replicate".to_string(),
            description: String::new(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "mcp-remote@latest".to_string(),
                "https://mcp.replicate.com/sse".to_string(),
            ],
            cwd: String::new(),
            env: HashMap::new(),
            lifecycle: None,
            stdio_protocol: crate::config::StdioProtocol::Auto,
            enabled: true,
        }
    }

    #[test]
    fn detects_mcp_remote_adapter() {
        let adapter = detect_known_adapter(&sample_server()).expect("adapter");
        assert_eq!(adapter.kind, KnownAdapterKind::McpRemote);
        assert!(adapter
            .resource_id
            .contains("https://mcp.replicate.com/sse"));
    }

    #[test]
    fn session_key_is_stable_across_env_order() {
        let mut left = sample_server();
        left.env.insert("B".to_string(), "2".to_string());
        left.env.insert("A".to_string(), "1".to_string());

        let mut right = sample_server();
        right.env.insert("A".to_string(), "1".to_string());
        right.env.insert("B".to_string(), "2".to_string());

        assert_eq!(
            session_key_for_server(&left, detect_known_adapter(&left).as_ref()),
            session_key_for_server(&right, detect_known_adapter(&right).as_ref())
        );
    }

    #[test]
    fn prepare_server_injects_controlled_dirs() {
        let store = AuthSessionStore::new_in(PathBuf::from("D:/tmp/auth-test"));
        let orchestrator = AuthOrchestrator::with_store(store);
        let prepared = orchestrator
            .prepare_server(&sample_server())
            .expect("prepare");

        assert_eq!(
            prepared.preferred_protocol,
            Some(NegotiatedStdioProtocol::JsonLines)
        );
        assert!(
            env_contains_key(&prepared.server.env, "MCP_REMOTE_CONFIG_DIR"),
            "expected managed auth dir"
        );
        assert!(
            env_contains_key(&prepared.server.env, "npm_config_cache"),
            "expected managed npm cache"
        );
    }

    #[test]
    fn auth_signal_extracts_authorize_url() {
        let event = detect_auth_signal(
            "Please authorize this client by visiting: https://example.com/authorize?client_id=1",
            AuthSignalSource::Stderr,
        );
        assert_eq!(
            event.authorize_url.as_deref(),
            Some("https://example.com/authorize?client_id=1")
        );
        assert!(event.score >= AUTH_SCORE_THRESHOLD);
    }

    #[tokio::test]
    async fn session_store_round_trip_and_clear() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AuthSessionStore::new_in(temp.path().to_path_buf());
        let state = ServerAuthState {
            status: AuthSessionStatus::Connected,
            authorize_url: Some("https://example.com/login".to_string()),
            last_success_at: Some(Utc::now()),
            last_updated_at: Some(Utc::now()),
            last_error: None,
            adapter_kind: Some("generic".to_string()),
            browser_opened: true,
            session_key: "abc".to_string(),
            session_dir: Some(temp.path().join("abc").display().to_string()),
        };

        store.save(&state).await.expect("save");
        let loaded = store.load("abc").await.expect("load").expect("state");
        assert_eq!(loaded.status, AuthSessionStatus::Connected);
        store.clear("abc").await.expect("clear");
        assert!(store.load("abc").await.expect("load after clear").is_none());
    }
}
