use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use mm_meta::decisions::{Decision, DecisionKey};
use mm_meta::protocol::{
    AuthenticatedBody, AuthenticatedResponse, ProtocolError, ProtocolQuery,
    QueryPayload, TransactionPayload, TransactionResponse,
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
        TransactionResponse::BatchApprovalStaged(s) => Ok(Json(serde_json::json!({
            "staged_releases": s.staged_releases,
            "staged_tracks": s.staged_tracks,
            "skipped_tracks": s.skipped_tracks,
            "skipped_releases": s.skipped_releases,
        }))),
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
    eprintln!("[WEB-TX] start label={:?}", body.label);
    let tr = send_tx(
        &state,
        token,
        TransactionPayload::Start { label: body.label },
    )
    .await?;
    eprintln!("[WEB-TX] start -> {tr:?}");
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
    eprintln!("[WEB-TX] add_decision key={}, label={:?}", body.key, body.decision.label);
    let tr = send_tx(
        &state,
        token,
        TransactionPayload::AddDecision {
            key: body.key,
            decision: body.decision,
        },
    )
    .await?;
    eprintln!("[WEB-TX] add_decision -> {tr:?}");
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

// ============================================================================
// POST /tx/approve-releases
// ============================================================================

#[derive(Deserialize)]
pub struct ApproveReleasesRequest {
    release_ids: Vec<String>,
}

/// Server-side bulk release approval. Single round-trip — the witch
/// loads review/staging data, builds decisions, and stages them all
/// in-process via `BatchApproveReleases`.
pub async fn approve_releases(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
    Json(body): Json<ApproveReleasesRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.release_ids.is_empty() {
        return Err(ApiError::BadRequest("no release IDs".into()));
    }

    let tr = send_tx(
        &state,
        token,
        TransactionPayload::BatchApproveReleases {
            release_ids: body.release_ids,
        },
    )
    .await?;
    match tr {
        TransactionResponse::BatchApprovalStaged(s) => Ok(Json(serde_json::json!({
            "staged_releases": s.staged_releases,
            "staged_tracks": s.staged_tracks,
            "skipped_tracks": s.skipped_tracks,
            "skipped_releases": s.skipped_releases,
        }))),
        TransactionResponse::Error(e) => Err(ProtocolError::Transaction(e).into()),
        other => Err(ApiError::Internal(format!(
            "unexpected response from BatchApproveReleases: {other:?}"
        ))),
    }
}

// ============================================================================
// POST /tx/stage-deploy
// ============================================================================

/// Server-side deploy staging: fetches deploy data, builds mutations using
/// PathResolver, and stages them into a transaction.
///
/// The web frontend should NOT build mutation paths — it sends only intent.
pub async fn stage_deploy(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
) -> Result<Json<serde_json::Value>, ApiError> {
    use mm_meta::domain_queries::GetDeployData;
    use mm_meta::paths::PathResolver;
    use mm_ui::deploy::prepare_deploy_decisions;

    // 1. Load deploy data.
    let deploy_resp = super::queries::send_query(
        &state, token.clone(), GetDeployData.into_payload(),
    ).await?;
    let deploy_data = GetDeployData::extract_response(deploy_resp);

    let total = deploy_data.total_operations();
    if total == 0 {
        return Err(ApiError::BadRequest("no deploy operations to stage".into()));
    }

    // 2. Load config for PathResolver.
    let config_resp = super::queries::send_query(
        &state, token.clone(), QueryPayload::Config,
    ).await?;
    let config = match config_resp {
        mm_meta::protocol::QueryResponse::Config(c) => *c,
        _ => return Err(ApiError::Internal("expected Config response".into())),
    };
    let resolver = PathResolver::from_config(&config);

    // 3. Build decisions server-side.
    let prepared = prepare_deploy_decisions(&deploy_data, &resolver);

    if prepared.decisions.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "no mutations produced (skipped {} files — likely missing library config)",
            prepared.skipped,
        )));
    }

    // 4. Stage transaction.
    let label = format!("Deploy {} operations", total);
    let _ = send_tx(&state, token.clone(), TransactionPayload::Discard).await;
    let tr = send_tx(&state, token.clone(), TransactionPayload::Start { label }).await?;
    if matches!(tr, TransactionResponse::Error(_)) {
        return tx_to_json(tr);
    }

    let n = prepared.decisions.len();
    for dd in prepared.decisions {
        let decision = Decision { label: dd.label, mutations: dd.mutations };
        let tr = send_tx(
            &state,
            token.clone(),
            TransactionPayload::AddDecision { key: dd.key, decision },
        ).await?;
        if matches!(tr, TransactionResponse::Error(_)) {
            return tx_to_json(tr);
        }
    }

    eprintln!("[WEB-TX] stage-deploy: {n} decisions staged, {total} ops, {} skipped", prepared.skipped);

    Ok(Json(serde_json::json!({
        "staged": n,
        "total_ops": total,
        "skipped": prepared.skipped,
    })))
}
