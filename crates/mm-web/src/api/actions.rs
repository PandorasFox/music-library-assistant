use axum::extract::State;
use axum::Json;

use mm_ui::protocol_binding::ProtocolBinding;

use crate::auth::BearerToken;
use crate::error::ApiError;
use crate::AppState;

// ============================================================================
// POST /actions/execute
// ============================================================================

/// Execute a typed protocol binding. The web client sends a serialized
/// `ProtocolBinding` and this handler runs the correct protocol flow.
///
/// `Transaction` bindings are display metadata only — the client builds
/// mutations locally and submits via `/tx/start` → `/tx/add`. The
/// `Transaction` arm returns an error directing callers to the tx API.
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

        ProtocolBinding::Transaction { .. } => Err(ApiError::BadRequest(
            "Transaction bindings are display metadata only; \
             use /tx/start + /tx/add to submit mutations"
                .into(),
        )),

        ProtocolBinding::Navigation => Ok(Json(serde_json::json!({"ok": true}))),
    }
}
