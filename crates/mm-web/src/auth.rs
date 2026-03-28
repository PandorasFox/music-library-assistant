use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use mm_meta::auth::SessionToken;

use crate::error::ApiError;

const COOKIE_NAME: &str = "mm_session";

/// Extractor that pulls a `SessionToken` from either:
/// 1. The `mm_session` cookie (preferred — set by login handler)
/// 2. The `Authorization: Bearer <base64>` header (fallback — for API clients)
pub struct BearerToken(pub SessionToken);

impl<S: Send + Sync> FromRequestParts<S> for BearerToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Try cookie first.
        if let Some(cookie_header) = parts.headers.get("cookie") {
            if let Ok(cookies) = cookie_header.to_str() {
                for cookie in cookies.split(';') {
                    let cookie = cookie.trim();
                    if let Some(value) = cookie.strip_prefix(COOKIE_NAME).and_then(|s| s.strip_prefix('=')) {
                        if let Ok(bytes) = STANDARD.decode(value) {
                            return Ok(BearerToken(SessionToken::from_bytes(bytes)));
                        }
                    }
                }
            }
        }

        // Fall back to Authorization header.
        let header = parts
            .headers
            .get("authorization")
            .ok_or_else(|| ApiError::Unauthorized("no session".into()))?
            .to_str()
            .map_err(|_| ApiError::Unauthorized("invalid Authorization header encoding".into()))?;

        let b64 = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| ApiError::Unauthorized("expected Bearer token".into()))?;

        let bytes = STANDARD
            .decode(b64)
            .map_err(|_| ApiError::Unauthorized("invalid base64 in token".into()))?;

        Ok(BearerToken(SessionToken::from_bytes(bytes)))
    }
}

/// Build a `Set-Cookie` header value for the session token.
pub fn session_cookie(b64_token: &str) -> String {
    // HttpOnly: not accessible to JS (XSS protection)
    // SameSite=Lax: sent on same-site navigations + top-level GET
    // Path=/: available to all routes
    // No Secure flag: mm-web runs behind a local reverse proxy,
    // the proxy terminates TLS if needed.
    format!("{COOKIE_NAME}={b64_token}; HttpOnly; SameSite=Lax; Path=/")
}

/// Build a `Set-Cookie` header value that expires the session cookie.
pub fn clear_session_cookie() -> String {
    format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}
