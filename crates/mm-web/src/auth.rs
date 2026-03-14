use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use mm_meta::auth::SessionToken;

use crate::error::ApiError;

/// Extractor that pulls a `SessionToken` from the `Authorization: Bearer <base64>` header.
pub struct BearerToken(pub SessionToken);

impl<S: Send + Sync> FromRequestParts<S> for BearerToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get("authorization")
            .ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?
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
