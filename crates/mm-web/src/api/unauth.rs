use axum::extract::State;
use axum::Json;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use mm_meta::protocol::{AuthResponse, UnauthenticatedBody, UnauthenticatedResponse};
use mm_meta::wire::{WireRequest, WireResponse};

use crate::error::ApiError;
use crate::AppState;

// ============================================================================
// POST /setup/check
// ============================================================================

#[derive(Serialize)]
pub struct SetupCheckResponse {
    needs_setup: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggested_root: Option<String>,
}

pub async fn setup_check(
    State(state): State<AppState>,
) -> Result<Json<SetupCheckResponse>, ApiError> {
    let req = WireRequest::Unauthenticated {
        request_id: 0,
        body: UnauthenticatedBody::SetupQuery,
    };
    let resp = state.conn.send(req).await?;

    match resp {
        WireResponse::Unauthenticated { result: Ok(UnauthenticatedResponse::SetupStatus {
            needs_setup, suggested_root,
        }), .. } => Ok(Json(SetupCheckResponse {
            needs_setup,
            suggested_root: suggested_root.map(|p| p.to_string_lossy().into_owned()),
        })),
        WireResponse::Unauthenticated { result: Err(e), .. } => Err(ApiError::Protocol(e)),
        _ => Err(ApiError::Internal("unexpected response type".into())),
    }
}

// ============================================================================
// POST /setup/complete
// ============================================================================

#[derive(Deserialize)]
pub struct SetupCompleteRequest {
    root: std::path::PathBuf,
    username: Option<String>,
    password: Option<String>,
}

#[derive(Serialize)]
pub struct OkResponse {
    ok: bool,
}

pub async fn setup_complete(
    State(state): State<AppState>,
    Json(body): Json<SetupCompleteRequest>,
) -> Result<Json<OkResponse>, ApiError> {
    let first_user = match (body.username, body.password) {
        (Some(u), Some(p)) => Some((u, p)),
        _ => None,
    };

    let req = WireRequest::Unauthenticated {
        request_id: 0,
        body: UnauthenticatedBody::CompleteSetup {
            root: body.root,
            first_user,
        },
    };
    let resp = state.conn.send(req).await?;

    match resp {
        WireResponse::Unauthenticated { result: Ok(UnauthenticatedResponse::SetupComplete), .. } => {
            Ok(Json(OkResponse { ok: true }))
        }
        WireResponse::Unauthenticated { result: Err(e), .. } => Err(ApiError::Protocol(e)),
        _ => Err(ApiError::Internal("unexpected response type".into())),
    }
}

// ============================================================================
// POST /auth/login
// ============================================================================

#[derive(Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

pub async fn logout() -> impl axum::response::IntoResponse {
    let cookie = crate::auth::clear_session_cookie();
    (
        [(axum::http::header::SET_COOKIE, cookie)],
        Json(serde_json::json!({ "ok": true })),
    )
}

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let req = WireRequest::Unauthenticated {
        request_id: 0,
        body: UnauthenticatedBody::Login {
            username: body.username,
            password: body.password,
        },
    };
    let resp = state.conn.send(req).await?;

    match resp {
        WireResponse::Unauthenticated { result: Ok(UnauthenticatedResponse::Auth(auth)), .. } => match auth {
            AuthResponse::Token(token) => {
                // Cache the token locally so WS connections can validate without
                // a Witch round-trip.
                state.register_session(token.as_bytes());
                let b64 = STANDARD.encode(token.as_bytes());

                // Set HttpOnly cookie + return token in body (cookie for browser, body for API clients).
                let cookie = crate::auth::session_cookie(&b64);
                Ok((
                    [(axum::http::header::SET_COOKIE, cookie)],
                    Json(serde_json::json!({ "token": b64 })),
                ))
            }
            AuthResponse::Failed(msg) => Err(ApiError::Unauthorized(msg)),
        },
        WireResponse::Unauthenticated { result: Err(e), .. } => Err(ApiError::Protocol(e)),
        _ => Err(ApiError::Internal("unexpected response type".into())),
    }
}
