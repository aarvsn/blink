//! Authentication layers.
//!
//! * `require_api_key` — panel-facing routes, constant-time `X-API-Key` check.
//! * `require_panel_token` — browser-facing filesystem routes; the token is
//!   forwarded to the panel for validation (with a short TTL cache).

use crate::error::AppError;
use crate::models::ValidateResponse;
use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use subtle::ConstantTimeEq;
use std::sync::Arc;

use crate::AppState;

/// Constant-time equality for secret material of possibly different lengths.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // Burn a comparison anyway so short/long probes cost the same.
        let _ = a.ct_eq(a);
        return false;
    }
    a.ct_eq(b).into()
}

/// Panel-facing middleware: `X-API-Key` must equal the configured node key.
pub async fn require_api_key(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let provided = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if provided.is_empty()
        || !constant_time_eq(provided.as_bytes(), state.config.api_key.as_bytes())
    {
        return AppError::Unauthorized("Invalid API key".into()).into_response();
    }

    next.run(req).await
}

/// Extract the panel token from `Authorization: Bearer` or `?token=`.
pub fn extract_token(
    headers: &axum::http::HeaderMap,
    query_token: Option<&str>,
) -> Option<String> {
    if let Some(value) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(value) = value.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }
    query_token
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
}

/// Filesystem middleware: validate the browser token with the panel.
/// Validations are cached briefly so bursts of file operations don't hammer
/// the panel; the cache is short enough that revoked tokens stop working fast.
pub async fn require_panel_token(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let query_token = req
        .uri()
        .query()
        .and_then(|q| {
            q.split('&').find_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                if k == "token" {
                    Some(percent_encoding::percent_decode_str(v).decode_utf8().ok()?.to_string())
                } else {
                    None
                }
            })
        });

    let token = match extract_token(req.headers(), query_token.as_deref()) {
        Some(t) => t,
        None => {
            return AppError::Unauthorized("Missing or invalid authorization token".into())
                .into_response()
        }
    };

    // serverId is always the first path segment under /api/v1/filesystem.
    let server_id = req
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();

    if server_id.is_empty() {
        return AppError::BadRequest("missing server id".into()).into_response();
    }

    match validate_cached(&state, &server_id, &token).await {
        Some(validation) if validation.validated => {
            let mut req = req;
            req.extensions_mut().insert(validation);
            next.run(req).await
        }
        _ => AppError::Forbidden("Invalid token or access denied".into()).into_response(),
    }
}

pub async fn validate_cached(
    state: &Arc<AppState>,
    server_id: &str,
    token: &str,
) -> Option<ValidateResponse> {
    if let Some(cached) = state.validation_cache.get(server_id, token).await {
        return Some(cached);
    }
    let validation = state.panel.validate_token(server_id, token).await?;
    state.validation_cache.put(server_id, token, validation.clone()).await;
    Some(validation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_lengths() {
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"", b"a"));
    }

    #[test]
    fn extracts_bearer() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer tok123"),
        );
        assert_eq!(extract_token(&headers, None).as_deref(), Some("tok123"));
        assert_eq!(extract_token(&headers, Some("q")).as_deref(), Some("tok123"));
        assert_eq!(extract_token(&axum::http::HeaderMap::new(), Some("q")).as_deref(), Some("q"));
        assert!(extract_token(&axum::http::HeaderMap::new(), None).is_none());
    }
}
