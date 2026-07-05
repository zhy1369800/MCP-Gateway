use axum::extract::State;
use axum::http::{header, Request};
use axum::middleware::Next;
use axum::response::Response;
use gateway_core::AppError;

use crate::response;
use crate::state::AppState;

pub async fn require_admin_auth(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let cfg = state.config_service.get_config().await;
    let token = extract_token(&request);
    if let Err(error) = enforce_bearer_token(
        token,
        cfg.security.admin.enabled,
        &cfg.security.admin.token,
    ) {
        return response::into_response(error);
    }
    next.run(request).await
}

pub async fn require_mcp_auth(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let cfg = state.config_service.get_config().await;
    let token = extract_token(&request);
    if let Err(error) = enforce_bearer_token(
        token,
        cfg.security.mcp.enabled,
        &cfg.security.mcp.token,
    ) {
        return response::into_response(error);
    }
    next.run(request).await
}

fn extract_token(request: &Request<axum::body::Body>) -> Option<String> {
    if let Some(raw) = request.headers().get(header::AUTHORIZATION) {
        if let Ok(value) = raw.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }
    if let Some(query) = request.uri().query() {
        if let Some(token_part) = query.split('&')
            .find(|p| p.starts_with("token="))
            .map(|p| p.strip_prefix("token=").unwrap_or(""))
        {
            if let Ok(decoded) = percent_encoding::percent_decode_str(token_part).decode_utf8() {
                return Some(decoded.to_string());
            }
        }
    }
    None
}

fn enforce_bearer_token(
    token: Option<String>,
    enabled: bool,
    expected_token: &str,
) -> Result<(), AppError> {
    if !enabled {
        return Ok(());
    }

    let Some(t) = token else {
        return Err(AppError::Unauthorized("missing bearer token".to_string()));
    };

    if t == expected_token {
        Ok(())
    } else {
        Err(AppError::Unauthorized("token mismatch".to_string()))
    }
}
