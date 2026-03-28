//! WebSocket endpoint for pushing server events to browser clients.
//!
//! The browser opens a WebSocket at `/ws?token=<base64>`. The token is
//! validated against mm-web's local session cache (populated at login) —
//! no round-trip to the Witch.
//!
//! The WS connection MUST NOT block on the Witch. It is a push channel
//! between mm-web and the browser — the Witch's availability must never
//! prevent the browser from connecting or receiving status updates.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Deserialize;

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
    let bytes = STANDARD
        .decode(&params.token)
        .map_err(|_| ApiError::Unauthorized("invalid base64 in token".into()))?;

    // Validate against local session cache — no Witch round-trip.
    if !state.is_valid_session(&bytes) {
        return Err(ApiError::Unauthorized("unknown session token".into()));
    }

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
