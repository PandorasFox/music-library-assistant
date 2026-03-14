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
}

pub async fn setup_check(
    State(state): State<AppState>,
) -> Result<Json<SetupCheckResponse>, ApiError> {
    let req = WireRequest::Unauthenticated(UnauthenticatedBody::SetupQuery);
    let resp = state.pool.send(&req).await?;

    match resp {
        WireResponse::Unauthenticated(Ok(UnauthenticatedResponse::SetupStatus {
            needs_setup,
        })) => Ok(Json(SetupCheckResponse { needs_setup })),
        WireResponse::Unauthenticated(Err(e)) => Err(ApiError::Protocol(e)),
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

    let req = WireRequest::Unauthenticated(UnauthenticatedBody::CompleteSetup {
        root: body.root,
        first_user,
    });
    let resp = state.pool.send(&req).await?;

    match resp {
        WireResponse::Unauthenticated(Ok(UnauthenticatedResponse::SetupComplete)) => {
            Ok(Json(OkResponse { ok: true }))
        }
        WireResponse::Unauthenticated(Err(e)) => Err(ApiError::Protocol(e)),
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

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let req = WireRequest::Unauthenticated(UnauthenticatedBody::Login {
        username: body.username,
        password: body.password,
    });
    let resp = state.pool.send(&req).await?;

    match resp {
        WireResponse::Unauthenticated(Ok(UnauthenticatedResponse::Auth(auth))) => match auth {
            AuthResponse::Token(token) => {
                let b64 = STANDARD.encode(token.as_bytes());
                Ok(Json(serde_json::json!({ "token": b64 })))
            }
            AuthResponse::Failed(msg) => Err(ApiError::Unauthorized(msg)),
        },
        WireResponse::Unauthenticated(Err(e)) => Err(ApiError::Protocol(e)),
        _ => Err(ApiError::Internal("unexpected response type".into())),
    }
}
