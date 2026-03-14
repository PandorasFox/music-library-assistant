use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use mm_meta::decisions::{Decision, DecisionKey};
use mm_meta::protocol::{
    AuthenticatedBody, AuthenticatedResponse, ProtocolError, TransactionPayload,
    TransactionResponse,
};
use mm_meta::wire::{WireRequest, WireResponse};

use crate::auth::BearerToken;
use crate::error::ApiError;
use crate::AppState;

/// Send an authenticated transaction request and extract the `TransactionResponse`.
pub(crate) async fn send_tx(
    state: &AppState,
    token: mm_meta::auth::SessionToken,
    payload: TransactionPayload,
) -> Result<TransactionResponse, ApiError> {
    let req = WireRequest::Authenticated {
        request_id: 0,
        token,
        body: Box::new(AuthenticatedBody::Transaction(payload)),
    };
    let resp = state.conn.send(req).await?;

    match resp {
        WireResponse::Authenticated { result, .. } => match *result {
            Ok(AuthenticatedResponse::Transaction(tr)) => Ok(tr),
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

fn tx_to_json(tr: TransactionResponse) -> Result<Json<serde_json::Value>, ApiError> {
    match tr {
        TransactionResponse::Ok => Ok(Json(serde_json::json!({"ok": true}))),
        TransactionResponse::Details(details) => serde_json::to_value(details)
            .map(Json)
            .map_err(|e| ApiError::Internal(e.to_string())),
        TransactionResponse::Discarded(_) => {
            Ok(Json(serde_json::json!({"discarded": true})))
        }
        TransactionResponse::Error(e) => Err(ProtocolError::Transaction(e).into()),
    }
}

// ============================================================================
// POST /tx/start
// ============================================================================

#[derive(Deserialize)]
pub struct StartRequest {
    label: String,
}

pub async fn start(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
    Json(body): Json<StartRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tr = send_tx(
        &state,
        token,
        TransactionPayload::Start { label: body.label },
    )
    .await?;
    tx_to_json(tr)
}

// ============================================================================
// POST /tx/add
// ============================================================================

#[derive(Deserialize)]
pub struct AddDecisionRequest {
    key: DecisionKey,
    decision: Decision,
}

pub async fn add_decision(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
    Json(body): Json<AddDecisionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tr = send_tx(
        &state,
        token,
        TransactionPayload::AddDecision {
            key: body.key,
            decision: body.decision,
        },
    )
    .await?;
    tx_to_json(tr)
}

// ============================================================================
// POST /tx/remove
// ============================================================================

#[derive(Deserialize)]
pub struct RemoveDecisionRequest {
    key: DecisionKey,
}

pub async fn remove_decision(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
    Json(body): Json<RemoveDecisionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tr = send_tx(
        &state,
        token,
        TransactionPayload::RemoveDecision { key: body.key },
    )
    .await?;
    tx_to_json(tr)
}

// ============================================================================
// GET /tx/details
// ============================================================================

pub async fn details(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tr = send_tx(&state, token, TransactionPayload::GetDetails).await?;
    tx_to_json(tr)
}

// ============================================================================
// POST /tx/confirm
// ============================================================================

pub async fn confirm(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tr = send_tx(&state, token, TransactionPayload::Confirm).await?;
    tx_to_json(tr)
}

// ============================================================================
// POST /tx/discard
// ============================================================================

pub async fn discard(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tr = send_tx(&state, token, TransactionPayload::Discard).await?;
    tx_to_json(tr)
}
