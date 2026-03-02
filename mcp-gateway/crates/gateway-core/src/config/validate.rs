use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;

use crate::error::AppError;

use super::model::GatewayConfig;

pub fn validate_config(cfg: &GatewayConfig) -> Result<(), AppError> {
    let listen_addr: SocketAddr = cfg
        .listen
        .parse()
        .map_err(|_| AppError::Validation(format!("invalid listen address: {}", cfg.listen)))?;

    if !cfg.allow_non_loopback && !listen_addr.ip().is_loopback() {
        return Err(AppError::Validation(
            "listen address must be loopback unless allowNonLoopback=true".to_string(),
        ));
    }

    validate_path(
        "transport.streamableHttp.basePath",
        &cfg.transport.streamable_http.base_path,
    )?;
    validate_path("transport.sse.basePath", &cfg.transport.sse.base_path)?;

    if cfg.security.admin.enabled && cfg.security.admin.token.trim().is_empty() {
        return Err(AppError::Validation(
            "security.admin.enabled=true requires non-empty security.admin.token".to_string(),
        ));
    }
    if cfg.security.mcp.enabled && cfg.security.mcp.token.trim().is_empty() {
        return Err(AppError::Validation(
            "security.mcp.enabled=true requires non-empty security.mcp.token".to_string(),
        ));
    }

    if cfg.defaults.request_timeout_ms < 1000 {
        return Err(AppError::Validation(
            "defaults.requestTimeoutMs must be >= 1000".to_string(),
        ));
    }
    if cfg.defaults.idle_ttl_ms < 1000 {
        return Err(AppError::Validation(
            "defaults.idleTtlMs must be >= 1000".to_string(),
        ));
    }
    if cfg.defaults.max_response_wait_iterations < 1 {
        return Err(AppError::Validation(
            "defaults.maxResponseWaitIterations must be >= 1".to_string(),
        ));
    }

    let mut names = HashSet::new();
    for server in &cfg.servers {
        if server.name.trim().is_empty() {
            return Err(AppError::Validation(
                "server.name cannot be empty".to_string(),
            ));
        }
        if !names.insert(server.name.clone()) {
            return Err(AppError::Validation(format!(
                "duplicate server.name: {}",
                server.name
            )));
        }
        if server.command.trim().is_empty() {
            return Err(AppError::Validation(format!(
                "server.command cannot be empty for {}",
                server.name
            )));
        }
    }

    if cfg.skills.server_name.trim().is_empty() {
        return Err(AppError::Validation(
            "skills.serverName cannot be empty".to_string(),
        ));
    }
    if cfg.skills.server_name.contains('/') || cfg.skills.server_name.contains('\\') {
        return Err(AppError::Validation(
            "skills.serverName cannot contain path separators".to_string(),
        ));
    }
    if cfg
        .servers
        .iter()
        .any(|server| server.name == cfg.skills.server_name)
    {
        return Err(AppError::Validation(format!(
            "skills.serverName conflicts with existing server: {}",
            cfg.skills.server_name
        )));
    }
    if cfg.skills.execution.timeout_ms < 1000 {
        return Err(AppError::Validation(
            "skills.execution.timeoutMs must be >= 1000".to_string(),
        ));
    }
    if cfg.skills.execution.max_output_bytes < 1024 {
        return Err(AppError::Validation(
            "skills.execution.maxOutputBytes must be >= 1024".to_string(),
        ));
    }
    if cfg.skills.policy.path_guard.enabled
        && cfg.skills.policy.path_guard.whitelist_dirs.is_empty()
    {
        return Err(AppError::Validation(
            "skills.policy.pathGuard.enabled=true requires non-empty whitelistDirs".to_string(),
        ));
    }
    for dir in &cfg.skills.policy.path_guard.whitelist_dirs {
        if !Path::new(dir).is_absolute() {
            return Err(AppError::Validation(format!(
                "skills.policy.pathGuard.whitelistDirs must be absolute paths: {dir}"
            )));
        }
    }

    let mut rule_ids = HashSet::new();
    for (idx, rule) in cfg.skills.policy.rules.iter().enumerate() {
        if rule.id.trim().is_empty() {
            return Err(AppError::Validation(format!(
                "skills.policy.rules[{idx}].id cannot be empty"
            )));
        }
        if !rule_ids.insert(rule.id.clone()) {
            return Err(AppError::Validation(format!(
                "duplicate skills.policy.rules id: {}",
                rule.id
            )));
        }
        if rule.command_tree.is_empty() && rule.contains.is_empty() {
            return Err(AppError::Validation(format!(
                "skills.policy.rules[{idx}] must have commandTree or contains"
            )));
        }
    }

    Ok(())
}

fn validate_path(name: &str, value: &str) -> Result<(), AppError> {
    if !value.starts_with('/') {
        return Err(AppError::Validation(format!(
            "{name} must start with '/': {value}"
        )));
    }
    if value.contains(' ') {
        return Err(AppError::Validation(format!(
            "{name} cannot contain spaces: {value}"
        )));
    }
    Ok(())
}
