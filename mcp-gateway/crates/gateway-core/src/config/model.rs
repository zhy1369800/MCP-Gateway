use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

use rand::distributions::{Alphanumeric, DistString};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::AppError;

use super::validate::validate_config;

const CONFIG_DIR_NAME: &str = "mcp-gateway";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Extension,
    General,
    #[default]
    Both,
}

impl Display for RunMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Extension => write!(f, "extension"),
            Self::General => write!(f, "general"),
            Self::Both => write!(f, "both"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleMode {
    #[default]
    Pooled,
    PerRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StdioProtocol {
    #[default]
    #[serde(
        alias = "content_length",
        alias = "contentLength",
        alias = "content-length",
        alias = "jsonl",
        alias = "json_lines",
        alias = "jsonLines",
        alias = "json-lines"
    )]
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default)]
    pub allow_non_loopback: bool,
    #[serde(default)]
    pub mode: RunMode,
    #[serde(default = "default_api_prefix")]
    pub api_prefix: String,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub transport: TransportConfig,
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
    #[serde(default)]
    pub skills: SkillsConfig,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            listen: default_listen(),
            allow_non_loopback: false,
            mode: RunMode::Both,
            api_prefix: default_api_prefix(),
            security: SecurityConfig::default(),
            transport: TransportConfig::default(),
            defaults: DefaultsConfig::default(),
            servers: Vec::new(),
            skills: SkillsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillsConfig {
    #[serde(default = "default_skills_server_name", skip_serializing)]
    pub server_name: String,
    #[serde(default = "default_builtin_skills_server_name", skip_serializing)]
    pub builtin_server_name: String,
    #[serde(default = "default_skills_roots")]
    pub roots: Vec<String>,
    #[serde(default)]
    pub policy: SkillsPolicyConfig,
    #[serde(default)]
    pub execution: SkillsExecutionConfig,
    #[serde(default)]
    pub builtin_tools: BuiltinToolsConfig,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            server_name: default_skills_server_name(),
            builtin_server_name: default_builtin_skills_server_name(),
            roots: default_skills_roots(),
            policy: SkillsPolicyConfig::default(),
            execution: SkillsExecutionConfig::default(),
            builtin_tools: BuiltinToolsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillsPolicyConfig {
    #[serde(default)]
    pub default_action: SkillPolicyAction,
    #[serde(default = "default_skills_command_rules")]
    pub rules: Vec<SkillCommandRule>,
    #[serde(default)]
    pub path_guard: SkillsPathGuardConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confirm_keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_keywords: Vec<String>,
}

impl Default for SkillsPolicyConfig {
    fn default() -> Self {
        Self {
            default_action: SkillPolicyAction::Allow,
            rules: default_skills_command_rules(),
            path_guard: SkillsPathGuardConfig::default(),
            confirm_keywords: Vec::new(),
            deny_keywords: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillPolicyAction {
    #[default]
    Allow,
    Confirm,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillsPathGuardConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub whitelist_dirs: Vec<String>,
    #[serde(default = "default_path_guard_violation_action")]
    pub on_violation: SkillPolicyAction,
}

impl Default for SkillsPathGuardConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            whitelist_dirs: Vec::new(),
            on_violation: default_path_guard_violation_action(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillCommandRule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub action: SkillPolicyAction,
    #[serde(default)]
    pub command_tree: Vec<String>,
    #[serde(default)]
    pub contains: Vec<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub reason_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillsExecutionConfig {
    #[serde(default = "default_skills_exec_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_skills_max_output_bytes")]
    pub max_output_bytes: usize,
}

impl Default for SkillsExecutionConfig {
    fn default() -> Self {
        Self {
            timeout_ms: default_skills_exec_timeout_ms(),
            max_output_bytes: default_skills_max_output_bytes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinToolsConfig {
    #[serde(default = "default_builtin_tool_enabled")]
    pub read_file: bool,
    #[serde(default = "default_builtin_tool_enabled")]
    pub shell_command: bool,
    #[serde(default = "default_builtin_tool_enabled")]
    pub multi_edit_file: bool,
    #[serde(default = "default_builtin_tool_enabled")]
    pub task_planning: bool,
    #[serde(default = "default_builtin_tool_enabled")]
    pub chrome_cdp: bool,
    #[serde(default = "default_builtin_tool_enabled")]
    pub chat_plus_adapter_debugger: bool,
    #[serde(default = "default_builtin_tool_disabled")]
    pub office_cli: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office_cli_path: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub shell_env: HashMap<String, String>,
}

impl Default for BuiltinToolsConfig {
    fn default() -> Self {
        Self {
            read_file: default_builtin_tool_enabled(),
            shell_command: default_builtin_tool_enabled(),
            multi_edit_file: default_builtin_tool_enabled(),
            task_planning: default_builtin_tool_enabled(),
            chrome_cdp: default_builtin_tool_enabled(),
            chat_plus_adapter_debugger: default_builtin_tool_enabled(),
            office_cli: default_builtin_tool_disabled(),
            office_cli_path: None,
            shell_env: HashMap::new(),
        }
    }
}

fn default_builtin_tool_enabled() -> bool {
    true
}

fn default_builtin_tool_disabled() -> bool {
    false
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecurityConfig {
    #[serde(default)]
    pub mcp: TokenConfig,
    #[serde(default = "default_admin_token_config")]
    pub admin: TokenConfig,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            mcp: TokenConfig {
                enabled: false,
                token: String::new(),
            },
            admin: default_admin_token_config(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransportConfig {
    #[serde(default = "default_streamable_http_path", rename = "streamableHttp")]
    pub streamable_http: TransportPath,
    #[serde(default = "default_sse_path")]
    pub sse: TransportPath,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            streamable_http: default_streamable_http_path(),
            sse: default_sse_path(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransportPath {
    #[serde(default)]
    pub base_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsConfig {
    #[serde(default)]
    pub lifecycle: LifecycleMode,
    #[serde(default = "default_idle_ttl_ms")]
    pub idle_ttl_ms: u64,
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_max_response_wait_iterations")]
    pub max_response_wait_iterations: u32,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            lifecycle: LifecycleMode::Pooled,
            idle_ttl_ms: default_idle_ttl_ms(),
            request_timeout_ms: default_request_timeout_ms(),
            max_retries: default_max_retries(),
            max_response_wait_iterations: default_max_response_wait_iterations(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub lifecycle: Option<LifecycleMode>,
    #[serde(default)]
    pub stdio_protocol: StdioProtocol,
    #[serde(default = "default_server_enabled")]
    pub enabled: bool,
}

impl ServerConfig {
    pub fn display_name(&self) -> String {
        if self.description.trim().is_empty() {
            self.name.clone()
        } else {
            self.description.clone()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScope {
    Admin,
    Mcp,
}

fn default_version() -> u32 {
    2
}

fn default_listen() -> String {
    "127.0.0.1:8765".to_string()
}

fn default_streamable_http_path() -> TransportPath {
    TransportPath {
        base_path: "/api/v2/mcp".to_string(),
    }
}

fn default_sse_path() -> TransportPath {
    TransportPath {
        base_path: "/api/v2/sse".to_string(),
    }
}

fn default_idle_ttl_ms() -> u64 {
    300_000
}

fn default_request_timeout_ms() -> u64 {
    60_000
}

fn default_max_retries() -> u32 {
    2
}

fn default_max_response_wait_iterations() -> u32 {
    100
}

fn default_admin_token_config() -> TokenConfig {
    TokenConfig {
        enabled: true,
        token: String::new(),
    }
}

fn default_api_prefix() -> String {
    "/api/v2".to_string()
}

fn default_server_enabled() -> bool {
    true
}

fn default_skills_server_name() -> String {
    "__skills__".to_string()
}

fn default_builtin_skills_server_name() -> String {
    "__builtin_skills__".to_string()
}

fn default_skills_roots() -> Vec<String> {
    Vec::new()
}

fn default_skills_command_rules() -> Vec<SkillCommandRule> {
    let mut rules = vec![
        SkillCommandRule {
            id: "deny-rm-root".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["rm".to_string()],
            contains: vec!["-rf".to_string(), "/".to_string()],
            reason: "Potential root destructive deletion".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-remove-item-root".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["remove-item".to_string()],
            contains: vec!["-recurse".to_string(), "c:\\".to_string()],
            reason: "Potential recursive deletion on drive root".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-sudo".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["sudo".to_string()],
            contains: Vec::new(),
            reason: "Privilege escalation command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-su".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["su".to_string()],
            contains: Vec::new(),
            reason: "User switching command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-doas".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["doas".to_string()],
            contains: Vec::new(),
            reason: "Privilege escalation command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-chmod".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["chmod".to_string()],
            contains: Vec::new(),
            reason: "Permission modification command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-chown".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["chown".to_string()],
            contains: Vec::new(),
            reason: "Ownership modification command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-chgrp".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["chgrp".to_string()],
            contains: Vec::new(),
            reason: "Group ownership modification command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-takeown".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["takeown".to_string()],
            contains: Vec::new(),
            reason: "Windows ownership takeover command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-icacls".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["icacls".to_string()],
            contains: Vec::new(),
            reason: "Windows ACL modification command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-reg".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["reg".to_string()],
            contains: Vec::new(),
            reason: "Registry modification command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-bcdedit".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["bcdedit".to_string()],
            contains: Vec::new(),
            reason: "Boot configuration command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-netsh".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["netsh".to_string()],
            contains: Vec::new(),
            reason: "Network configuration command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-runas".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["runas".to_string()],
            contains: Vec::new(),
            reason: "Privilege escalation command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-taskkill".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["taskkill".to_string()],
            contains: Vec::new(),
            reason: "Process termination command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-diskpart".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["diskpart".to_string()],
            contains: Vec::new(),
            reason: "Disk partition command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-format".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["format".to_string()],
            contains: Vec::new(),
            reason: "Disk formatting command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-certutil".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["certutil".to_string()],
            contains: Vec::new(),
            reason: "Download or certificate manipulation command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-bitsadmin".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["bitsadmin".to_string()],
            contains: Vec::new(),
            reason: "Background transfer command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-msiexec".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["msiexec".to_string()],
            contains: Vec::new(),
            reason: "Installer execution command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-regsvr32".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["regsvr32".to_string()],
            contains: Vec::new(),
            reason: "Binary registration command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-rundll32".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["rundll32".to_string()],
            contains: Vec::new(),
            reason: "Dynamic library execution command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-schtasks".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["schtasks".to_string()],
            contains: Vec::new(),
            reason: "Task scheduler command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-sc".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["sc".to_string()],
            contains: Vec::new(),
            reason: "Service controller command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-systemctl".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["systemctl".to_string()],
            contains: Vec::new(),
            reason: "Service controller command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-service".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["service".to_string()],
            contains: Vec::new(),
            reason: "Service controller command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-pkexec".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["pkexec".to_string()],
            contains: Vec::new(),
            reason: "Privilege escalation command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-kill".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["kill".to_string()],
            contains: Vec::new(),
            reason: "Process termination command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-pkill".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["pkill".to_string()],
            contains: Vec::new(),
            reason: "Process termination command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-killall".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["killall".to_string()],
            contains: Vec::new(),
            reason: "Process termination command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-apt".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["apt".to_string()],
            contains: Vec::new(),
            reason: "Package manager command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-apt-get".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["apt-get".to_string()],
            contains: Vec::new(),
            reason: "Package manager command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-yum".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["yum".to_string()],
            contains: Vec::new(),
            reason: "Package manager command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-dnf".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["dnf".to_string()],
            contains: Vec::new(),
            reason: "Package manager command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-pacman".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["pacman".to_string()],
            contains: Vec::new(),
            reason: "Package manager command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-zypper".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["zypper".to_string()],
            contains: Vec::new(),
            reason: "Package manager command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-apk".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["apk".to_string()],
            contains: Vec::new(),
            reason: "Package manager command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-brew".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["brew".to_string()],
            contains: Vec::new(),
            reason: "Package manager command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-winget".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["winget".to_string()],
            contains: Vec::new(),
            reason: "Package manager command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-choco".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["choco".to_string()],
            contains: Vec::new(),
            reason: "Package manager command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-iptables".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["iptables".to_string()],
            contains: Vec::new(),
            reason: "Firewall command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-nft".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["nft".to_string()],
            contains: Vec::new(),
            reason: "Firewall command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-ufw".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["ufw".to_string()],
            contains: Vec::new(),
            reason: "Firewall command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-ip".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["ip".to_string()],
            contains: Vec::new(),
            reason: "Network configuration command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-ifconfig".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["ifconfig".to_string()],
            contains: Vec::new(),
            reason: "Network configuration command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-route".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["route".to_string()],
            contains: Vec::new(),
            reason: "Routing configuration command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-dd".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["dd".to_string()],
            contains: Vec::new(),
            reason: "Raw disk write command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-mkfs".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["mkfs".to_string()],
            contains: Vec::new(),
            reason: "Filesystem creation command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-fdisk".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["fdisk".to_string()],
            contains: Vec::new(),
            reason: "Disk partition command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-parted".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["parted".to_string()],
            contains: Vec::new(),
            reason: "Disk partition command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-mount".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["mount".to_string()],
            contains: Vec::new(),
            reason: "Mount command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-umount".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["umount".to_string()],
            contains: Vec::new(),
            reason: "Unmount command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-chattr".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["chattr".to_string()],
            contains: Vec::new(),
            reason: "File attribute modification command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-setfacl".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["setfacl".to_string()],
            contains: Vec::new(),
            reason: "ACL modification command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-useradd".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["useradd".to_string()],
            contains: Vec::new(),
            reason: "User management command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-usermod".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["usermod".to_string()],
            contains: Vec::new(),
            reason: "User management command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-userdel".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["userdel".to_string()],
            contains: Vec::new(),
            reason: "User management command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-groupadd".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["groupadd".to_string()],
            contains: Vec::new(),
            reason: "Group management command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-groupdel".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["groupdel".to_string()],
            contains: Vec::new(),
            reason: "Group management command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-launchctl".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["launchctl".to_string()],
            contains: Vec::new(),
            reason: "macOS service controller command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-defaults".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["defaults".to_string()],
            contains: Vec::new(),
            reason: "macOS preferences write command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-spctl".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["spctl".to_string()],
            contains: Vec::new(),
            reason: "macOS security policy command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-csrutil".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["csrutil".to_string()],
            contains: Vec::new(),
            reason: "macOS SIP command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-security".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["security".to_string()],
            contains: Vec::new(),
            reason: "macOS keychain command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-osascript".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["osascript".to_string()],
            contains: Vec::new(),
            reason: "AppleScript execution command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-diskutil".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["diskutil".to_string()],
            contains: Vec::new(),
            reason: "Disk utility command is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-powershell".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["powershell".to_string()],
            contains: Vec::new(),
            reason: "Nested shell launch is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-pwsh".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["pwsh".to_string()],
            contains: Vec::new(),
            reason: "Nested shell launch is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-cmd".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["cmd".to_string()],
            contains: Vec::new(),
            reason: "Nested shell launch is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-bash-c".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["bash".to_string()],
            contains: vec!["-c".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-bash-lc".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["bash".to_string()],
            contains: vec!["-lc".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-sh-c".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["sh".to_string()],
            contains: vec!["-c".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-sh-lc".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["sh".to_string()],
            contains: vec!["-lc".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-zsh-c".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["zsh".to_string()],
            contains: vec!["-c".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-zsh-lc".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["zsh".to_string()],
            contains: vec!["-lc".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-dash-c".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["dash".to_string()],
            contains: vec!["-c".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-dash-lc".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["dash".to_string()],
            contains: vec!["-lc".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-ksh-c".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["ksh".to_string()],
            contains: vec!["-c".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-ksh-lc".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["ksh".to_string()],
            contains: vec!["-lc".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-fish-c".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["fish".to_string()],
            contains: vec!["-c".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-tcsh-c".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["tcsh".to_string()],
            contains: vec!["-c".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-csh-c".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["csh".to_string()],
            contains: vec!["-c".to_string()],
            reason: "Shell wrapper command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-invoke-expression".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["invoke-expression".to_string()],
            contains: Vec::new(),
            reason: "Dynamic command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-iex".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["iex".to_string()],
            contains: Vec::new(),
            reason: "Dynamic command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "deny-invoke-command".to_string(),
            action: SkillPolicyAction::Deny,
            command_tree: vec!["invoke-command".to_string()],
            contains: Vec::new(),
            reason: "Remote command execution is blocked".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-curl".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["curl".to_string()],
            contains: Vec::new(),
            reason: "Network download command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-wget".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["wget".to_string()],
            contains: Vec::new(),
            reason: "Network download command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-invoke-webrequest".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["invoke-webrequest".to_string()],
            contains: Vec::new(),
            reason: "Network download command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-irm".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["irm".to_string()],
            contains: Vec::new(),
            reason: "Network download command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-set-content".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["set-content".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-add-content".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["add-content".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-clear-content".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["clear-content".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-out-file".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["out-file".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-tee".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["tee".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-sed".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["sed".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-awk".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["awk".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-perl".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["perl".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-ed".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["ed".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-ex".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["ex".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-vi".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["vi".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-vim".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["vim".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-nvim".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["nvim".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-nano".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["nano".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-notepad".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["notepad".to_string()],
            contains: Vec::new(),
            reason: "Text editing command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-rm".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["rm".to_string()],
            contains: Vec::new(),
            reason: "File deletion command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-del".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["del".to_string()],
            contains: Vec::new(),
            reason: "File deletion command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-rmdir".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["rmdir".to_string()],
            contains: Vec::new(),
            reason: "Directory deletion command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-remove-item".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["remove-item".to_string()],
            contains: Vec::new(),
            reason: "PowerShell deletion command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-unlink".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["unlink".to_string()],
            contains: Vec::new(),
            reason: "File unlink command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-mv".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["mv".to_string()],
            contains: Vec::new(),
            reason: "File move command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-move".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["move".to_string()],
            contains: Vec::new(),
            reason: "File move command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-move-item".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["move-item".to_string()],
            contains: Vec::new(),
            reason: "File move command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-cp".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["cp".to_string()],
            contains: Vec::new(),
            reason: "File copy command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-copy".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["copy".to_string()],
            contains: Vec::new(),
            reason: "File copy command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-copy-item".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["copy-item".to_string()],
            contains: Vec::new(),
            reason: "File copy command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-rename".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["rename".to_string()],
            contains: Vec::new(),
            reason: "File rename command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-ren".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["ren".to_string()],
            contains: Vec::new(),
            reason: "File rename command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-rename-item".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["rename-item".to_string()],
            contains: Vec::new(),
            reason: "File rename command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-new-item".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["new-item".to_string()],
            contains: Vec::new(),
            reason: "File/directory creation command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-mkdir".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["mkdir".to_string()],
            contains: Vec::new(),
            reason: "Directory creation command requires confirmation".to_string(),
            reason_key: String::new(),
        },
        SkillCommandRule {
            id: "confirm-touch".to_string(),
            action: SkillPolicyAction::Confirm,
            command_tree: vec!["touch".to_string()],
            contains: Vec::new(),
            reason: "File creation/timestamp update command requires confirmation".to_string(),
            reason_key: String::new(),
        },
    ];
    populate_skill_rule_reason_keys(&mut rules);
    rules
}

fn populate_skill_rule_reason_keys(rules: &mut [SkillCommandRule]) {
    for rule in rules {
        if rule.reason_key.trim().is_empty() {
            rule.reason_key = infer_skill_rule_reason_key(&rule.id, &rule.reason).to_string();
        } else {
            rule.reason_key = rule.reason_key.trim().to_ascii_lowercase();
        }
    }
}

fn infer_skill_rule_reason_key(id: &str, reason: &str) -> &'static str {
    match id {
        "deny-rm-root" => "root_destructive_deletion",
        "deny-remove-item-root" => "drive_root_recursive_deletion",
        "confirm-route" => "routing_configuration",
        "confirm-unlink" => "file_unlink",
        _ => match reason {
            "Privilege escalation command is blocked" => "privilege_escalation",
            "User switching command is blocked" => "user_switching",
            "Permission modification command is blocked" => "permission_modification",
            "Ownership modification command is blocked" => "ownership_modification",
            "Group ownership modification command is blocked" => "group_ownership_modification",
            "Windows ownership takeover command is blocked" => "windows_ownership_takeover",
            "Windows ACL modification command is blocked" => "windows_acl_modification",
            "Registry modification command is blocked" => "registry_modification",
            "Boot configuration command is blocked" => "boot_configuration",
            "Network configuration command requires confirmation" => "network_configuration",
            "Process termination command requires confirmation" => "process_termination",
            "Disk partition command is blocked" => "disk_partition",
            "Disk formatting command is blocked" => "disk_formatting",
            "Download or certificate manipulation command is blocked" => "download_or_certificate",
            "Background transfer command is blocked" => "background_transfer",
            "Installer execution command requires confirmation" => "installer_execution",
            "Binary registration command is blocked" => "binary_registration",
            "Dynamic library execution command is blocked" => "dynamic_library_execution",
            "Task scheduler command is blocked" => "task_scheduler",
            "Service controller command requires confirmation" => "service_controller",
            "Package manager command requires confirmation" => "package_manager",
            "Firewall command is blocked" => "firewall",
            "Raw disk write command is blocked" => "raw_disk_write",
            "Filesystem creation command is blocked" => "filesystem_creation",
            "Mount command requires confirmation" => "mount",
            "Unmount command requires confirmation" => "unmount",
            "File attribute modification command is blocked" => "file_attribute_modification",
            "ACL modification command is blocked" => "acl_modification",
            "User management command is blocked" => "user_management",
            "Group management command is blocked" => "group_management",
            "macOS service controller command requires confirmation" => "macos_service_controller",
            "macOS preferences write command requires confirmation" => "macos_preferences",
            "macOS security policy command is blocked" => "macos_security_policy",
            "macOS SIP command is blocked" => "macos_sip",
            "macOS keychain command is blocked" => "macos_keychain",
            "AppleScript execution command requires confirmation" => "applescript_execution",
            "Disk utility command is blocked" => "disk_utility",
            "Nested shell launch is blocked" => "nested_shell",
            "Shell wrapper command execution is blocked" => "shell_wrapper",
            "Dynamic command execution is blocked" => "dynamic_command_execution",
            "Remote command execution is blocked" => "remote_command_execution",
            "Network download command requires confirmation"
            | "Network download command is blocked" => "network_download",
            "Text editing command requires confirmation" => "text_editing",
            "File deletion command requires confirmation" => "file_deletion",
            "Directory deletion command requires confirmation" => "directory_deletion",
            "PowerShell deletion command requires confirmation" => "powershell_deletion",
            "File move command requires confirmation" => "file_move",
            "File copy command requires confirmation" => "file_copy",
            "File rename command requires confirmation" => "file_rename",
            "File/directory creation command requires confirmation" => "file_or_directory_creation",
            "Directory creation command requires confirmation" => "directory_creation",
            "File creation/timestamp update command requires confirmation" => "file_touch",
            _ => "custom",
        },
    }
}

fn default_path_guard_violation_action() -> SkillPolicyAction {
    SkillPolicyAction::Allow
}

fn default_skills_exec_timeout_ms() -> u64 {
    60_000
}

fn default_skills_max_output_bytes() -> usize {
    128 * 1024
}

pub fn generate_token() -> String {
    Alphanumeric.sample_string(&mut rand::rngs::OsRng, 40)
}

pub fn default_config_path() -> Result<PathBuf, AppError> {
    let mut base =
        dirs::config_dir().ok_or_else(|| AppError::Internal("Invalid config path".to_string()))?;
    base.push(CONFIG_DIR_NAME);
    base.push("config.v2.json");
    Ok(base)
}

pub fn load_config_from_path(path: &Path) -> Result<GatewayConfig, AppError> {
    let text = fs::read_to_string(path)?;
    let mut cfg: GatewayConfig = serde_json::from_str(&text)?;
    normalize_config_in_place(&mut cfg);
    validate_config(&cfg)?;
    Ok(cfg)
}

pub fn init_default_config(path: &Path, mode: RunMode) -> Result<GatewayConfig, AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut cfg = GatewayConfig {
        mode,
        ..GatewayConfig::default()
    };
    cfg.security.admin.token = generate_token();
    cfg.security.mcp.token = generate_token();
    normalize_config_in_place(&mut cfg);
    validate_config(&cfg)?;
    save_config_atomic(path, &cfg)?;
    Ok(cfg)
}

pub fn save_config_atomic(path: &Path, cfg: &GatewayConfig) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Internal("Invalid config path".to_string()))?;
    fs::create_dir_all(parent)?;

    let tmp_name = format!(".config-{}.tmp", Uuid::new_v4());
    let tmp_path = parent.join(tmp_name);
    let data = serde_json::to_vec_pretty(cfg)?;

    fs::write(&tmp_path, data)?;

    #[cfg(target_os = "windows")]
    {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }

    fs::rename(tmp_path, path)?;
    Ok(())
}

pub fn rotate_token(path: &Path, scope: TokenScope) -> Result<String, AppError> {
    let mut cfg = load_config_from_path(path)?;
    let token = generate_token();
    match scope {
        TokenScope::Admin => cfg.security.admin.token = token.clone(),
        TokenScope::Mcp => cfg.security.mcp.token = token.clone(),
    }
    save_config_atomic(path, &cfg)?;
    Ok(token)
}

pub fn apply_runtime_overrides(
    cfg: &mut GatewayConfig,
    mode: Option<RunMode>,
    listen: Option<String>,
) {
    if let Some(m) = mode {
        cfg.mode = m;
    }
    if let Some(l) = listen {
        cfg.listen = l;
    }
    normalize_config_in_place(cfg);
}

pub fn normalize_config_in_place(cfg: &mut GatewayConfig) {
    cfg.version = 2;
    cfg.listen = cfg.listen.trim().to_string();
    cfg.transport.streamable_http.base_path =
        normalize_path(&cfg.transport.streamable_http.base_path, "/api/v2/mcp");
    cfg.transport.sse.base_path = normalize_path(&cfg.transport.sse.base_path, "/api/v2/sse");

    cfg.security.admin.token = cfg.security.admin.token.trim().to_string();
    cfg.security.mcp.token = cfg.security.mcp.token.trim().to_string();

    for server in &mut cfg.servers {
        server.name = server.name.trim().to_string();
        server.description = server.description.trim().to_string();
        server.command = server.command.trim().to_string();
        server.cwd = server.cwd.trim().to_string();
        server.args = server
            .args
            .iter()
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();

        server.env = server
            .env
            .iter()
            .filter_map(|(k, v)| {
                let key = k.trim().to_string();
                let value = v.trim().to_string();
                if key.is_empty() || value.is_empty() {
                    None
                } else {
                    Some((key, value))
                }
            })
            .collect();
    }

    cfg.skills.server_name = default_skills_server_name();
    cfg.skills.builtin_server_name = default_builtin_skills_server_name();

    cfg.skills.roots = cfg
        .skills
        .roots
        .iter()
        .map(|root| root.trim().to_string())
        .filter(|root| !root.is_empty())
        .collect();

    cfg.skills.policy.rules = cfg
        .skills
        .policy
        .rules
        .iter()
        .map(|rule| SkillCommandRule {
            id: rule.id.trim().to_string(),
            action: rule.action.clone(),
            command_tree: rule
                .command_tree
                .iter()
                .map(|node| node.trim().to_ascii_lowercase())
                .filter(|node| !node.is_empty())
                .collect(),
            contains: rule
                .contains
                .iter()
                .map(|token| token.trim().to_ascii_lowercase())
                .filter(|token| !token.is_empty())
                .collect(),
            reason: rule.reason.trim().to_string(),
            reason_key: rule.reason_key.trim().to_ascii_lowercase(),
        })
        .filter(|rule| !rule.command_tree.is_empty() || !rule.contains.is_empty())
        .collect();

    cfg.skills.policy.path_guard.whitelist_dirs = cfg
        .skills
        .policy
        .path_guard
        .whitelist_dirs
        .iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();

    cfg.skills.policy.confirm_keywords = cfg
        .skills
        .policy
        .confirm_keywords
        .iter()
        .map(|keyword| keyword.trim().to_ascii_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .collect();

    cfg.skills.policy.deny_keywords = cfg
        .skills
        .policy
        .deny_keywords
        .iter()
        .map(|keyword| keyword.trim().to_ascii_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .collect();

    if !cfg.skills.policy.confirm_keywords.is_empty() {
        for (idx, keyword) in cfg.skills.policy.confirm_keywords.iter().enumerate() {
            cfg.skills.policy.rules.push(SkillCommandRule {
                id: format!("legacy-confirm-{}", idx + 1),
                action: SkillPolicyAction::Confirm,
                command_tree: Vec::new(),
                contains: vec![keyword.clone()],
                reason: format!("Legacy confirm keyword: {keyword}"),
                reason_key: "legacy_confirm_keyword".to_string(),
            });
        }
    }
    if !cfg.skills.policy.deny_keywords.is_empty() {
        for (idx, keyword) in cfg.skills.policy.deny_keywords.iter().enumerate() {
            cfg.skills.policy.rules.push(SkillCommandRule {
                id: format!("legacy-deny-{}", idx + 1),
                action: SkillPolicyAction::Deny,
                command_tree: Vec::new(),
                contains: vec![keyword.clone()],
                reason: format!("Legacy deny keyword: {keyword}"),
                reason_key: "legacy_deny_keyword".to_string(),
            });
        }
    }

    cfg.skills.policy.confirm_keywords.clear();
    cfg.skills.policy.deny_keywords.clear();
    populate_skill_rule_reason_keys(&mut cfg.skills.policy.rules);
}

fn normalize_path(input: &str, fallback: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    let mut path = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    while path.ends_with('/') && path.len() > 1 {
        path.pop();
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let mut cfg = GatewayConfig::default();
        cfg.security.admin.token = "abc".to_string();
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn normalize_path_defaults() {
        let mut cfg = GatewayConfig::default();
        cfg.transport.streamable_http.base_path = "mcp/".to_string();
        normalize_config_in_place(&mut cfg);
        assert_eq!(cfg.transport.streamable_http.base_path, "/mcp");
        assert_eq!(cfg.version, 2);
    }

    #[test]
    fn default_skills_rules_include_sensitive_denies_and_editor_confirmations() {
        let rules = default_skills_command_rules();
        let check_action = |id: &str, action: SkillPolicyAction| {
            let rule = rules
                .iter()
                .find(|item| item.id == id)
                .unwrap_or_else(|| panic!("missing rule: {id}"));
            assert_eq!(rule.action, action);
        };

        check_action("deny-sudo", SkillPolicyAction::Deny);
        check_action("deny-runas", SkillPolicyAction::Deny);
        check_action("deny-pkexec", SkillPolicyAction::Deny);
        check_action("confirm-launchctl", SkillPolicyAction::Confirm);
        check_action("deny-invoke-expression", SkillPolicyAction::Deny);
        check_action("confirm-curl", SkillPolicyAction::Confirm);
        check_action("confirm-wget", SkillPolicyAction::Confirm);
        check_action("confirm-invoke-webrequest", SkillPolicyAction::Confirm);
        check_action("confirm-irm", SkillPolicyAction::Confirm);
        check_action("deny-bash-lc", SkillPolicyAction::Deny);
        check_action("confirm-set-content", SkillPolicyAction::Confirm);
        check_action("confirm-sed", SkillPolicyAction::Confirm);
        check_action("confirm-vim", SkillPolicyAction::Confirm);
        assert!(rules.iter().all(|rule| !rule.reason_key.is_empty()));
    }

    #[test]
    fn legacy_stdio_protocol_values_fold_into_auto() {
        let content_length: StdioProtocol =
            serde_json::from_str(r#""content_length""#).expect("parse content_length");
        let json_lines: StdioProtocol =
            serde_json::from_str(r#""json_lines""#).expect("parse json_lines");

        assert_eq!(content_length, StdioProtocol::Auto);
        assert_eq!(json_lines, StdioProtocol::Auto);
    }

    #[test]
    fn default_skills_execution_timeout_is_one_minute() {
        assert_eq!(SkillsExecutionConfig::default().timeout_ms, 60_000);
    }
}
