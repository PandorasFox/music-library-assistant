//! WebSocket endpoint for pushing server events to browser clients.
//!
//! The browser opens a WebSocket at `/ws?token=<base64>`. The server validates
//! the token, then subscribes to `WitchClient::subscribe_events()` and forwards
//! `WitchEvent` frames as JSON text messages.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Deserialize;

use mm_meta::auth::SessionToken;
use mm_meta::protocol::QueryPayload;

use crate::error::ApiError;
use crate::AppState;

#[derive(Deserialize)]
pub struct WsParams {
    token: String,
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(params): Query<WsParams>,
) -> Result<impl IntoResponse, ApiError> {
    // Decode and validate token before upgrading the connection.
    let bytes = STANDARD
        .decode(&params.token)
        .map_err(|_| ApiError::Unauthorized("invalid base64 in token".into()))?;
    let token = SessionToken::from_bytes(bytes);

    // Verify the session is valid by sending a status query through the Witch.
    super::queries::send_query(&state, token, QueryPayload::Status).await?;

    Ok(ws.on_upgrade(move |socket| handle_ws(socket, state)))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut event_rx = state.conn.subscribe_events();

    loop {
        match event_rx.recv().await {
            Ok(event) => {
                let json = match serde_json::to_string(&event) {
                    Ok(j) => j,
                    Err(_) => continue,
                };
                if socket.send(Message::Text(json.into())).await.is_err() {
                    return;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                // Client fell behind — stale snapshots skipped, next recv is fresh
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return; // Witch shut down — drop socket
            }
        }
    }
}
