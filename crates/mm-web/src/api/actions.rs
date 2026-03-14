use axum::extract::State;
use axum::Json;

use mm_meta::db_types::Zone;
use mm_meta::decisions::{Decision, DecisionKey};
use mm_meta::domain_queries::{DomainQueryPayload, DomainQueryResult, GetIntakeConfirmation};
use mm_meta::protocol::{QueryPayload, QueryResponse, TransactionPayload, TransactionResponse};
use mm_meta::views::startup_organize::IntakeSource;
use mm_ui::protocol_binding::ProtocolBinding;

use crate::auth::BearerToken;
use crate::error::ApiError;
use crate::AppState;

// ============================================================================
// POST /actions/execute
// ============================================================================

/// Execute a typed protocol binding. The web client sends a serialized
/// `ProtocolBinding` and this handler runs the correct protocol flow.
pub async fn execute(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
    Json(binding): Json<ProtocolBinding>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match binding {
        ProtocolBinding::Command(cmd) => {
            let cr = super::commands::send_cmd(&state, token, cmd).await?;
            super::commands::cmd_to_json(cr)
        }

        ProtocolBinding::Transaction {
            decision_key,
            label,
        } => execute_transaction(&state, token, decision_key, label).await,

        ProtocolBinding::Navigation => Ok(Json(serde_json::json!({"ok": true}))),
    }
}

/// Transitional shim: auto-confirm for IntakeIndex.
///
/// Once the web client has proper transaction review, this entire function
/// goes away — the client will build mutations locally and submit via
/// `/tx/start` → `/tx/add`, then navigate to `/transaction` for review.
async fn execute_transaction(
    state: &AppState,
    token: mm_meta::auth::SessionToken,
    decision_key: DecisionKey,
    label: String,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mutations = match decision_key {
        DecisionKey::IntakeIndex => query_intake_mutations(state, token.clone()).await?,
        _ => {
            return Err(ApiError::BadRequest(
                "only IntakeIndex is supported via execute_transaction; \
                 other actions should use the transaction API directly"
                    .into(),
            ));
        }
    };

    if mutations.is_empty() {
        return Ok(Json(
            serde_json::json!({"ok": true, "message": "nothing to index"}),
        ));
    }

    // Start transaction.
    let tr = super::transactions::send_tx(
        state,
        token.clone(),
        TransactionPayload::Start {
            label: label.clone(),
        },
    )
    .await?;
    if let TransactionResponse::Error(e) = tr {
        return Err(mm_meta::protocol::ProtocolError::Transaction(e).into());
    }

    // Add decision.
    let decision = Decision {
        label: label.clone(),
        mutations,
    };
    let tr = super::transactions::send_tx(
        state,
        token.clone(),
        TransactionPayload::AddDecision {
            key: decision_key,
            decision,
        },
    )
    .await?;
    if let TransactionResponse::Error(e) = tr {
        return Err(mm_meta::protocol::ProtocolError::Transaction(e).into());
    }

    // Confirm.
    let tr = super::transactions::send_tx(state, token, TransactionPayload::Confirm).await?;
    match tr {
        TransactionResponse::Error(e) => {
            Err(mm_meta::protocol::ProtocolError::Transaction(e).into())
        }
        _ => Ok(Json(serde_json::json!({"ok": true}))),
    }
}

/// Query for unindexed files and return the mutations to index them.
async fn query_intake_mutations(
    state: &AppState,
    token: mm_meta::auth::SessionToken,
) -> Result<Vec<mm_meta::mutations::Mutation>, ApiError> {
    let payload = QueryPayload::Domain(Box::new(DomainQueryPayload::GetIntakeConfirmation(
        GetIntakeConfirmation {
            source: IntakeSource::Health,
            zone: Some(Zone::Corpus),
        },
    )));

    let qr = super::queries::send_query(state, token, payload).await?;

    match qr {
        QueryResponse::Domain(DomainQueryResult::GetIntakeConfirmation(Some(intake))) => {
            Ok(intake.create_index_mutations())
        }
        QueryResponse::Domain(DomainQueryResult::GetIntakeConfirmation(None)) => Ok(vec![]),
        _ => Err(ApiError::Internal(
            "unexpected query response for intake confirmation".into(),
        )),
    }
}
