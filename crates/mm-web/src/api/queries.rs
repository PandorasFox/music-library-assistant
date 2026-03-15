use std::collections::HashMap;

use axum::body::Bytes;
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
pub(crate) async fn send_query(
    state: &AppState,
    token: mm_meta::auth::SessionToken,
    payload: QueryPayload,
) -> Result<QueryResponse, ApiError> {
    let req = WireRequest::Authenticated {
        request_id: 0,
        token,
        body: Box::new(AuthenticatedBody::Query(payload)),
    };
    let resp = state.conn.send(req).await?;

    match resp {
        WireResponse::Authenticated { result, .. } => match *result {
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
// GET /config-kdl
// ============================================================================

pub async fn config_kdl(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
) -> Result<Json<serde_json::Value>, ApiError> {
    let qr = send_query(&state, token, QueryPayload::ConfigKdl).await?;
    match qr {
        QueryResponse::ConfigKdl(s) => to_json(s),
        _ => Err(ApiError::Internal("expected ConfigKdl response".into())),
    }
}

// ============================================================================
// GET|POST /queries/{name}
// ============================================================================

/// Strips the outer enum tag from a serde-serialized `DomainQueryResult`,
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

/// Unified handler for domain queries. Accepts both GET (query params for
/// simple queries) and POST (JSON body for queries with complex parameters
/// like Vecs or nested structs).
pub async fn domain_query(
    State(state): State<AppState>,
    BearerToken(token): BearerToken,
    Path(name): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let payload = build_domain_payload(&name, &params, &body)?;

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

/// Map a kebab-case path segment to a `DomainQueryPayload`.
///
/// Simple queries parse scalar params from the query string.
/// Complex queries (Vec params, nested structs) deserialize from the JSON body.
fn build_domain_payload(
    name: &str,
    params: &HashMap<String, String>,
    body: &[u8],
) -> Result<DomainQueryPayload, ApiError> {
    match name {
        // ================================================================
        // Summary queries (unit structs)
        // ================================================================
        "insights" => Ok(DomainQueryPayload::GetInsights(GetInsights)),
        "inbox-overview" => Ok(DomainQueryPayload::GetInboxOverview(GetInboxOverview)),
        "deploy-status" => Ok(DomainQueryPayload::GetDeployStatus(GetDeployStatus)),
        "edit-history" => Ok(DomainQueryPayload::GetEditHistory(GetEditHistory)),
        "external-matches" => {
            Ok(DomainQueryPayload::GetExternalMatches(GetExternalMatches))
        }
        "packing-dirs" => Ok(DomainQueryPayload::GetPackingDirs(GetPackingDirs)),

        // ================================================================
        // Wave 1: detail queries
        // ================================================================
        "oob-sync-files" => Ok(DomainQueryPayload::GetOobSyncFiles(GetOobSyncFiles)),
        "oob-files-bucketed" => {
            Ok(DomainQueryPayload::GetOobFilesBucketed(GetOobFilesBucketed))
        }
        // oob-conflict-by-bucket?bucket=MtimeOnly
        "oob-conflict-by-bucket" => {
            let bucket = parse_enum(params, "bucket")?;
            Ok(DomainQueryPayload::GetOobConflictByBucket(
                GetOobConflictByBucket { bucket },
            ))
        }
        "moved-files" => Ok(DomainQueryPayload::GetMovedFiles(GetMovedFiles)),
        "missing-album-single-signals" => Ok(
            DomainQueryPayload::GetMissingAlbumSingleSignals(GetMissingAlbumSingleSignals),
        ),
        "packing-knots" => Ok(DomainQueryPayload::GetPackingKnots(GetPackingKnots)),
        "packing-inode-paths" => {
            Ok(DomainQueryPayload::GetPackingInodePaths(GetPackingInodePaths))
        }

        // edit-history-export?session_id=... (optional)
        "edit-history-export" => {
            let session_id = params.get("session_id").cloned();
            Ok(DomainQueryPayload::GetEditHistoryExport(
                GetEditHistoryExport { session_id },
            ))
        }

        // compound-signal-groups?zone=Corpus&safe_only=true&tag_filter=ARTIST
        "compound-signal-groups" => {
            let zone = parse_enum(params, "zone")?;
            let safe_only = params
                .get("safe_only")
                .map(|v| v == "true")
                .unwrap_or(false);
            let tag_filter = params.get("tag_filter").cloned();
            Ok(DomainQueryPayload::GetCompoundSignalGroups(
                GetCompoundSignalGroups {
                    zone,
                    safe_only,
                    tag_filter,
                },
            ))
        }

        // ================================================================
        // Wave 2: signal key queries
        // ================================================================
        "inconsistent-album-artist-keys" => Ok(
            DomainQueryPayload::GetInconsistentAlbumArtistKeys(
                GetInconsistentAlbumArtistKeys,
            ),
        ),

        // tag-canonicity-keys?zone=Corpus&tag_filter=ARTIST
        "tag-canonicity-keys" => {
            let zone = parse_enum(params, "zone")?;
            let tag_filter = params.get("tag_filter").cloned();
            Ok(DomainQueryPayload::GetTagCanonicityKeys(
                GetTagCanonicityKeys { zone, tag_filter },
            ))
        }

        // disc-extraction-data?map_letters_to_numbers=true
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

        // ================================================================
        // Wave 3: modal init loaders
        // ================================================================
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

        // inbox-corpus-match-data?bitrate_fuzz_percent=5.0
        "inbox-corpus-match-data" => {
            let bitrate_fuzz_percent = parse_param(params, "bitrate_fuzz_percent")?;
            Ok(DomainQueryPayload::GetInboxCorpusMatchData(
                GetInboxCorpusMatchData {
                    bitrate_fuzz_percent,
                },
            ))
        }

        // deploy-data (GET with no body = config None, POST with JSON body = config Some)
        "deploy-data" => {
            let config = if body.is_empty() {
                None
            } else {
                Some(from_body(body)?)
            };
            Ok(DomainQueryPayload::GetDeployData(GetDeployData { config }))
        }

        // manual-review-data?kind=RedundantDuplicate
        "manual-review-data" => {
            let kind = parse_enum(params, "kind")?;
            Ok(DomainQueryPayload::GetManualReviewData(
                GetManualReviewData { kind },
            ))
        }

        // corpus-tags?inode=N
        "corpus-tags" => {
            let inode = parse_param(params, "inode")?;
            Ok(DomainQueryPayload::GetCorpusTags(GetCorpusTags { inode }))
        }

        // packing-browser-data?category_prefix=...
        "packing-browser-data" => {
            let category_prefix = require_param(params, "category_prefix")?;
            Ok(DomainQueryPayload::GetPackingBrowserData(
                GetPackingBrowserData { category_prefix },
            ))
        }

        // unsolved-packing-data?category=...
        "unsolved-packing-data" => {
            let category = require_param(params, "category")?;
            Ok(DomainQueryPayload::GetUnsolvedPackingData(
                GetUnsolvedPackingData { category },
            ))
        }

        // ================================================================
        // Wave 4: composite queries
        // ================================================================

        // POST body: { "inodes": [1, 2, 3], "zone": "Corpus" }
        "audio-files-by-inodes" => {
            let q: GetAudioFilesByInodes = from_body(body)?;
            Ok(DomainQueryPayload::GetAudioFilesByInodes(q))
        }

        "missing-tag-audio-files" => Ok(DomainQueryPayload::GetMissingTagAudioFiles(
            GetMissingTagAudioFiles,
        )),

        // all-audio-files-with-tags?zone=Corpus&include_library=false
        "all-audio-files-with-tags" => {
            let zone = parse_enum(params, "zone")?;
            let include_library = params
                .get("include_library")
                .map(|v| v == "true")
                .unwrap_or(false);
            Ok(DomainQueryPayload::GetAllAudioFilesWithTags(
                GetAllAudioFilesWithTags {
                    zone,
                    include_library,
                },
            ))
        }

        // session-edit-detail?session_id=...
        "session-edit-detail" => {
            let session_id = require_param(params, "session_id")?;
            Ok(DomainQueryPayload::GetSessionEditDetail(
                GetSessionEditDetail { session_id },
            ))
        }

        // POST body: { "queries": [[123, "ARTIST"], [456, "ALBUM"]] }
        "current-tag-values" => {
            let q: GetCurrentTagValues = from_body(body)?;
            Ok(DomainQueryPayload::GetCurrentTagValues(q))
        }

        // ================================================================
        // Wave 5
        // ================================================================

        // intake-confirmation?source=Startup&zone=Corpus (zone optional)
        "intake-confirmation" => {
            let source = parse_enum(params, "source")?;
            let zone = parse_optional_enum(params, "zone")?;
            Ok(DomainQueryPayload::GetIntakeConfirmation(
                GetIntakeConfirmation { source, zone },
            ))
        }

        // POST body: { "group": { "tag_name": "...", "compound_value": "...", "inodes": [...] }, "zone": "Corpus" }
        "compound-split-group-data" => {
            let q: GetCompoundSplitGroupData = from_body(body)?;
            Ok(DomainQueryPayload::GetCompoundSplitGroupData(q))
        }

        // tag-canonicity-signal-data?signal_key=...&kind=TagCanonicity
        "tag-canonicity-signal-data" => {
            let signal_key = require_param(params, "signal_key")?;
            let kind = parse_enum(params, "kind")?;
            Ok(DomainQueryPayload::GetTagCanonicitySignalData(
                GetTagCanonicitySignalData { signal_key, kind },
            ))
        }

        // POST body: { "recording_ids": ["...", "..."], "preferred_locales": ["en"] }
        "recording-batch-data" => {
            let q: GetRecordingBatchData = from_body(body)?;
            Ok(DomainQueryPayload::GetRecordingBatchData(q))
        }

        // POST body: { "release_ids": [...], "recording_ids": [...], "inodes": [...] }
        "release-staging-data" => {
            let q: GetReleaseStagingData = from_body(body)?;
            Ok(DomainQueryPayload::GetReleaseStagingData(q))
        }

        // tag-editor-files?rel_path=/some/path&mode=Directory
        "tag-editor-files" => {
            let rel_path = require_param(params, "rel_path")?.into();
            let mode = parse_enum(params, "mode")?;
            Ok(DomainQueryPayload::GetTagEditorFiles(GetTagEditorFiles {
                rel_path,
                mode,
            }))
        }

        // POST body: { full Config object }
        "inbox-organize-data" => {
            let config = from_body(body)?;
            Ok(DomainQueryPayload::GetInboxOrganizeData(
                GetInboxOrganizeData { config },
            ))
        }

        // POST body: { "inodes": [1, 2, 3], "zone": "Corpus" }
        "file-tag-values" => {
            let q: GetFileTagValues = from_body(body)?;
            Ok(DomainQueryPayload::GetFileTagValues(q))
        }

        // ================================================================
        // Web file browser & search
        // ================================================================

        // directory-listing?zone=Corpus&parent=some/path (parent optional)
        "directory-listing" => {
            let zone = parse_enum(params, "zone")?;
            let parent = params.get("parent").cloned();
            Ok(DomainQueryPayload::GetDirectoryListing(
                GetDirectoryListing { zone, parent },
            ))
        }

        // search-corpus?query=radiohead&limit=200 (limit optional, default 200)
        "search-corpus" => {
            let query = require_param(params, "query")?;
            let limit = params
                .get("limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(200);
            Ok(DomainQueryPayload::SearchCorpusFiles(
                SearchCorpusFiles { query, limit },
            ))
        }

        // ================================================================
        // Packed resolution queries (cluster-nav)
        // ================================================================

        // tag-canonicity-resolution?tag_name=ARTIST&zone=Corpus&filter_existing_canonicals=true
        "tag-canonicity-resolution" => {
            let tag_name = require_param(params, "tag_name")?;
            let zone = parse_enum(params, "zone")?;
            let filter_existing_canonicals = params
                .get("filter_existing_canonicals")
                .map(|v| v == "true")
                .unwrap_or(false);
            Ok(DomainQueryPayload::GetTagCanonicityResolution(
                GetTagCanonicityResolution {
                    tag_name,
                    zone,
                    filter_existing_canonicals,
                },
            ))
        }

        // compound-split-resolution?tag_name=GENRE&zone=Corpus&safe_only=true
        "compound-split-resolution" => {
            let tag_name = require_param(params, "tag_name")?;
            let zone = parse_enum(params, "zone")?;
            let safe_only = params
                .get("safe_only")
                .map(|v| v == "true")
                .unwrap_or(false);
            Ok(DomainQueryPayload::GetCompoundSplitResolution(
                GetCompoundSplitResolution {
                    tag_name,
                    zone,
                    safe_only,
                },
            ))
        }

        _ => Err(ApiError::BadRequest(format!(
            "unknown query: '{name}'"
        ))),
    }
}

// ============================================================================
// Parameter parsing helpers
// ============================================================================

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

/// Parse a serde enum variant name from a query string parameter.
/// E.g., `?zone=Corpus` → `Zone::Corpus`.
fn parse_enum<T: serde::de::DeserializeOwned>(
    params: &HashMap<String, String>,
    key: &str,
) -> Result<T, ApiError> {
    let s = params
        .get(key)
        .ok_or_else(|| ApiError::BadRequest(format!("missing '{key}' parameter")))?;
    serde_json::from_value(serde_json::Value::String(s.clone()))
        .map_err(|e| ApiError::BadRequest(format!("invalid '{key}': {e}")))
}

/// Parse an optional serde enum variant name from a query string parameter.
fn parse_optional_enum<T: serde::de::DeserializeOwned>(
    params: &HashMap<String, String>,
    key: &str,
) -> Result<Option<T>, ApiError> {
    match params.get(key) {
        Some(s) => serde_json::from_value(serde_json::Value::String(s.clone()))
            .map(Some)
            .map_err(|e| ApiError::BadRequest(format!("invalid '{key}': {e}"))),
        None => Ok(None),
    }
}

/// Deserialize the request body as a JSON-encoded query struct.
fn from_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest(
            "JSON body required for this query".into(),
        ));
    }
    serde_json::from_slice(body)
        .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
}
