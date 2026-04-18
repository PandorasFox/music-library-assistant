use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use mm_meta::protocol::ProtocolError;

/// Unified error type for all API endpoints.
pub enum ApiError {
    /// Mapped from a Witch protocol error.
    Protocol(ProtocolError),
    /// Socket transport failure (Witch unreachable, broken pipe, etc.).
    Transport(String),
    /// Bad HTTP request (missing params, invalid format).
    BadRequest(String),
    /// Missing or invalid Authorization header.
    Unauthorized(String),
    /// Endpoint not yet implemented.
    NotImplemented,
    /// Server-side serialization or logic error.
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::Protocol(ref e) => match e {
                ProtocolError::InvalidSession => (StatusCode::UNAUTHORIZED, e.to_string()),
                ProtocolError::Unauthorized => (StatusCode::FORBIDDEN, e.to_string()),
                ProtocolError::NotReady => (StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
                ProtocolError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
                ProtocolError::Transaction(_) => (StatusCode::CONFLICT, e.to_string()),
            },
            ApiError::Transport(msg) => (StatusCode::BAD_GATEWAY, msg),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            ApiError::NotImplemented => {
                (StatusCode::NOT_IMPLEMENTED, "not implemented".into())
            }
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let mut response = (status, Json(json!({"error": message}))).into_response();

        // Clear the session cookie on auth failures so the browser stops
        // sending a stale token (e.g. after container restart).
        if status == StatusCode::UNAUTHORIZED {
            if let Ok(val) = HeaderValue::from_str(&crate::auth::clear_session_cookie()) {
                response.headers_mut().insert("set-cookie", val);
            }
        }

        response
    }
}

impl From<ProtocolError> for ApiError {
    fn from(e: ProtocolError) -> Self {
        ApiError::Protocol(e)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Transport(e.to_string())
    }
}
