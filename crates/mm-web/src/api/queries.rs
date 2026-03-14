use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::Json;

use mm_meta::domain_queries::*;
use mm_meta::protocol::{
    AuthenticatedBody, AuthenticatedResponse, QueryPayload, QueryResponse,
};
use mm_meta::wire::{WireRequest, WireResponse};

use crate::auth::BearerToken;
use crate::error::ApiError;
use crate::AppState;

/// Send an authenticated query and extract the `QueryResponse`.
async fn send_query(
    state: &AppState,
    token: mm_meta::auth::SessionToken,
    payload: QueryPayload,
) -> Result<QueryResponse, ApiError> {
    let req = WireRequest::Authenticated {
        token,
        body: Box::new(AuthenticatedBody::Query(payload)),
    };
    let resp = state.pool.send(&req).await?;

    match resp {
        WireResponse::Authenticated(result) => match *result {
            Ok(AuthenticatedResponse::Query(qr)) => Ok(*qr),
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

fn to_json<T: serde::Serialize>(val: T) -> Result<Json<serde_json::Value>, ApiError> {
    serde_json::to_value(val)
        .map(Json)
        .map_err(|e| ApiError::Internal(e.to_string()))
}

// ============================================================================
// GET /status
// ============================================================================

pub async fn status(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
) -> Result<Json<serde_json::Value>, ApiError> {
    let qr = send_query(&state, token, QueryPayload::Status).await?;
    match qr {
        QueryResponse::Status(s) => to_json(s),
        _ => Err(ApiError::Internal("expected Status response".into())),
    }
}

// ============================================================================
// GET /config
// ============================================================================

pub async fn config(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
) -> Result<Json<serde_json::Value>, ApiError> {
    let qr = send_query(&state, token, QueryPayload::Config).await?;
    match qr {
        QueryResponse::Config(c) => to_json(*c),
        _ => Err(ApiError::Internal("expected Config response".into())),
    }
}

// ============================================================================
// GET /queries/{name}
// ============================================================================

/// Strips the outer enum tag from a serde-serialized DomainQueryResult,
/// returning just the inner data. The externally-tagged representation
/// is `{"VariantName": inner}` — we extract `inner`.
fn unwrap_domain_result(result: DomainQueryResult) -> Result<serde_json::Value, ApiError> {
    let value =
        serde_json::to_value(result).map_err(|e| ApiError::Internal(e.to_string()))?;
    match value {
        serde_json::Value::Object(map) => Ok(map
            .into_iter()
            .next()
            .map(|(_, v)| v)
            .unwrap_or(serde_json::Value::Null)),
        other => Ok(other),
    }
}

pub async fn domain_query(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
    Path(name): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let payload = build_domain_payload(&name, &params)?;

    let qr = send_query(
        &state,
        token,
        QueryPayload::Domain(Box::new(payload)),
    )
    .await?;

    match qr {
        QueryResponse::Domain(result) => unwrap_domain_result(result).map(Json),
        _ => Err(ApiError::Internal("expected Domain response".into())),
    }
}

/// Map a kebab-case path segment + query params to a `DomainQueryPayload`.
fn build_domain_payload(
    name: &str,
    params: &HashMap<String, String>,
) -> Result<DomainQueryPayload, ApiError> {
    match name {
        // Summary queries (unit structs)
        "insights" => Ok(DomainQueryPayload::GetInsights(GetInsights)),
        "inbox-overview" => Ok(DomainQueryPayload::GetInboxOverview(GetInboxOverview)),
        "deploy-status" => Ok(DomainQueryPayload::GetDeployStatus(GetDeployStatus)),
        "edit-history" => Ok(DomainQueryPayload::GetEditHistory(GetEditHistory)),
        "external-matches" => {
            Ok(DomainQueryPayload::GetExternalMatches(GetExternalMatches))
        }
        "packing-dirs" => Ok(DomainQueryPayload::GetPackingDirs(GetPackingDirs)),

        // Wave 1 unit structs
        "oob-sync-files" => Ok(DomainQueryPayload::GetOobSyncFiles(GetOobSyncFiles)),
        "oob-files-bucketed" => {
            Ok(DomainQueryPayload::GetOobFilesBucketed(GetOobFilesBucketed))
        }
        "moved-files" => Ok(DomainQueryPayload::GetMovedFiles(GetMovedFiles)),
        "missing-album-single-signals" => Ok(
            DomainQueryPayload::GetMissingAlbumSingleSignals(GetMissingAlbumSingleSignals),
        ),
        "packing-knots" => Ok(DomainQueryPayload::GetPackingKnots(GetPackingKnots)),
        "packing-inode-paths" => {
            Ok(DomainQueryPayload::GetPackingInodePaths(GetPackingInodePaths))
        }

        // Wave 2 unit structs
        "inconsistent-album-artist-keys" => Ok(
            DomainQueryPayload::GetInconsistentAlbumArtistKeys(
                GetInconsistentAlbumArtistKeys,
            ),
        ),

        // Wave 3 unit structs (modal loaders)
        "missing-file-data" => {
            Ok(DomainQueryPayload::GetMissingFileData(GetMissingFileData))
        }
        "missing-directory-data" => Ok(DomainQueryPayload::GetMissingDirectoryData(
            GetMissingDirectoryData,
        )),
        "corrupt-file-data" => {
            Ok(DomainQueryPayload::GetCorruptFileData(GetCorruptFileData))
        }
        "subpar-duplicate-data" => Ok(DomainQueryPayload::GetSubparDuplicateData(
            GetSubparDuplicateData,
        )),
        "directory-cluster-data" => Ok(DomainQueryPayload::GetDirectoryClusterData(
            GetDirectoryClusterData,
        )),
        "release-overlap-data" => Ok(DomainQueryPayload::GetReleaseOverlapData(
            GetReleaseOverlapData,
        )),
        "shit-format-data" => {
            Ok(DomainQueryPayload::GetShitFormatData(GetShitFormatData))
        }

        // Wave 4 unit struct
        "missing-tag-audio-files" => Ok(DomainQueryPayload::GetMissingTagAudioFiles(
            GetMissingTagAudioFiles,
        )),

        // Parameterized: corpus-tags?inode=N
        "corpus-tags" => {
            let inode = parse_param::<i64>(params, "inode")?;
            Ok(DomainQueryPayload::GetCorpusTags(GetCorpusTags { inode }))
        }

        // Parameterized: edit-history-export?session_id=... (optional)
        "edit-history-export" => {
            let session_id = params.get("session_id").cloned();
            Ok(DomainQueryPayload::GetEditHistoryExport(
                GetEditHistoryExport { session_id },
            ))
        }

        // Parameterized: disc-extraction-data?map_letters_to_numbers=true
        "disc-extraction-data" => {
            let map_letters_to_numbers = params
                .get("map_letters_to_numbers")
                .map(|v| v == "true")
                .unwrap_or(false);
            Ok(DomainQueryPayload::GetDiscExtractionData(
                GetDiscExtractionData {
                    map_letters_to_numbers,
                },
            ))
        }

        // Parameterized: session-edit-detail?session_id=...
        "session-edit-detail" => {
            let session_id = require_param(params, "session_id")?;
            Ok(DomainQueryPayload::GetSessionEditDetail(
                GetSessionEditDetail { session_id },
            ))
        }

        // Parameterized: packing-browser-data?category_prefix=...
        "packing-browser-data" => {
            let category_prefix = require_param(params, "category_prefix")?;
            Ok(DomainQueryPayload::GetPackingBrowserData(
                GetPackingBrowserData { category_prefix },
            ))
        }

        // Parameterized: unsolved-packing-data?category=...
        "unsolved-packing-data" => {
            let category = require_param(params, "category")?;
            Ok(DomainQueryPayload::GetUnsolvedPackingData(
                GetUnsolvedPackingData { category },
            ))
        }

        _ => Err(ApiError::NotImplemented),
    }
}

fn parse_param<T: std::str::FromStr>(
    params: &HashMap<String, String>,
    key: &str,
) -> Result<T, ApiError> {
    params
        .get(key)
        .ok_or_else(|| ApiError::BadRequest(format!("missing '{key}' parameter")))?
        .parse()
        .map_err(|_| ApiError::BadRequest(format!("invalid '{key}' parameter")))
}

fn require_param(params: &HashMap<String, String>, key: &str) -> Result<String, ApiError> {
    params
        .get(key)
        .cloned()
        .ok_or_else(|| ApiError::BadRequest(format!("missing '{key}' parameter")))
}
