use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use mm_meta::protocol::{
    AuthenticatedBody, AuthenticatedResponse, BackgroundTask, CommandPayload, CommandResponse,
};
use mm_meta::wire::{WireRequest, WireResponse};

use crate::auth::BearerToken;
use crate::error::ApiError;
use crate::AppState;

/// Send an authenticated command and extract the `CommandResponse`.
async fn send_cmd(
    state: &AppState,
    token: mm_meta::auth::SessionToken,
    payload: CommandPayload,
) -> Result<CommandResponse, ApiError> {
    let req = WireRequest::Authenticated {
        token,
        body: Box::new(AuthenticatedBody::Command(Box::new(payload))),
    };
    let resp = state.pool.send(&req).await?;

    match resp {
        WireResponse::Authenticated(result) => match *result {
            Ok(AuthenticatedResponse::Command(cr)) => Ok(cr),
            Ok(_) => Err(ApiError::Internal(
                "unexpected authenticated response variant".into(),
            )),
            Err(e) => Err(e.into()),
        },
        _ => Err(ApiError::Internal(
            "unexpected wire response type".into(),
        )),
    }
}

fn cmd_to_json(cr: CommandResponse) -> Result<Json<serde_json::Value>, ApiError> {
    match cr {
        CommandResponse::Ok => Ok(Json(serde_json::json!({"ok": true}))),
        CommandResponse::Goodbye => Ok(Json(serde_json::json!({"goodbye": true}))),
    }
}

// ============================================================================
// POST /commands/queue-task
// ============================================================================

#[derive(Deserialize)]
pub struct QueueTaskRequest {
    task: BackgroundTask,
}

pub async fn queue_task(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
    Json(body): Json<QueueTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cr = send_cmd(&state, token, CommandPayload::QueueTask(body.task)).await?;
    cmd_to_json(cr)
}

// ============================================================================
// POST /commands/jettison
// ============================================================================

#[derive(Deserialize)]
pub struct JettisonRequest {
    session_id: Option<String>,
}

pub async fn jettison(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
    Json(body): Json<JettisonRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cr = send_cmd(
        &state,
        token,
        CommandPayload::JettisonEditHistory {
            session_id: body.session_id,
        },
    )
    .await?;
    cmd_to_json(cr)
}

// ============================================================================
// POST /commands/shutdown
// ============================================================================

pub async fn shutdown(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cr = send_cmd(&state, token, CommandPayload::Shutdown).await?;
    cmd_to_json(cr)
}
