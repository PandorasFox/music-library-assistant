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

// ============================================================================
// WebQuery — typed parameter extraction per query struct
// ============================================================================

/// Each domain query struct implements this to parse itself from HTTP request
/// parameters. The dispatch below matches on `DomainQueryRoute` (the typed
/// route enum from mm-meta) and delegates to each struct's `from_web`.
trait WebQuery: Sized {
    fn from_web(params: &HashMap<String, String>, body: &[u8]) -> Result<Self, ApiError>;
}

/// Unit query structs: no parameters, just construct Self.
macro_rules! unit_web_query {
    ($($ty:ty),+ $(,)?) => {
        $(impl WebQuery for $ty {
            fn from_web(_: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
                Ok(Self)
            }
        })+
    };
}

/// Body-deserialized query structs: parse entire struct from JSON body.
macro_rules! body_web_query {
    ($($ty:ty),+ $(,)?) => {
        $(impl WebQuery for $ty {
            fn from_web(_: &HashMap<String, String>, body: &[u8]) -> Result<Self, ApiError> {
                from_body(body)
            }
        })+
    };
}

/// Exhaustive dispatch on `DomainQueryRoute`: each variant delegates to the
/// corresponding query struct's `WebQuery::from_web` implementation.
/// Adding a new query to `domain_query_protocol!` forces a new arm here.
macro_rules! dispatch_route {
    ($route:expr, $params:expr, $body:expr; $( $variant:ident ),+ $(,)?) => {
        match $route {
            $( DomainQueryRoute::$variant => Ok(DomainQueryPayload::$variant(
                <$variant as WebQuery>::from_web($params, $body)?
            )), )+
        }
    };
}

// -- Unit queries (no parameters) --

unit_web_query!(
    GetInsights, GetInboxOverview, GetDeployStatus, GetEditHistory,
    GetExternalMatches, GetPackingDirs,
    GetOobSyncFiles, GetOobFilesBucketed, GetMovedFiles,
    GetMissingAlbumSingleSignals, GetPackingKnots, GetPackingInodePaths,
    GetMissingFileData, GetMissingDirectoryData, GetCorruptFileData,
    GetSubparDuplicateData, GetDirectoryClusterData, GetReleaseOverlapData,
    GetShitFormatData, GetMissingTagAudioFiles,
);

#[allow(deprecated)]
unit_web_query!(GetInconsistentAlbumArtistKeys);

// -- Body-deserialized queries --

body_web_query!(
    GetAudioFilesByInodes, GetCurrentTagValues, GetRecordingBatchData,
    GetReleaseStagingData, GetInboxOrganizeData, GetFileTagValues,
    SearchWithConditions,
);

#[allow(deprecated)]
body_web_query!(GetCompoundSplitGroupData);

// -- Param-parsed queries (custom extraction per struct) --

impl WebQuery for GetOobConflictByBucket {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { bucket: parse_enum(params, "bucket")? })
    }
}

impl WebQuery for GetEditHistoryExport {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { session_id: params.get("session_id").cloned() })
    }
}

#[allow(deprecated)]
impl WebQuery for GetCompoundSignalGroups {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self {
            zone: parse_enum(params, "zone")?,
            safe_only: parse_bool(params, "safe_only"),
            tag_filter: params.get("tag_filter").cloned(),
        })
    }
}

#[allow(deprecated)]
impl WebQuery for GetTagCanonicityKeys {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self {
            zone: parse_enum(params, "zone")?,
            tag_filter: params.get("tag_filter").cloned(),
        })
    }
}

impl WebQuery for GetDiscExtractionData {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { map_letters_to_numbers: parse_bool(params, "map_letters_to_numbers") })
    }
}

impl WebQuery for GetInboxCorpusMatchData {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { bitrate_fuzz_percent: parse_param(params, "bitrate_fuzz_percent")? })
    }
}

impl WebQuery for GetDeployData {
    fn from_web(_: &HashMap<String, String>, body: &[u8]) -> Result<Self, ApiError> {
        let config = if body.is_empty() { None } else { Some(from_body(body)?) };
        Ok(Self { config })
    }
}

impl WebQuery for GetManualReviewData {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { kind: parse_enum(params, "kind")? })
    }
}

impl WebQuery for GetCorpusTags {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { inode: parse_param(params, "inode")? })
    }
}

impl WebQuery for GetPackingBrowserData {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { category_prefix: require_param(params, "category_prefix")? })
    }
}

impl WebQuery for GetUnsolvedPackingData {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { category: require_param(params, "category")? })
    }
}

impl WebQuery for GetAllAudioFilesWithTags {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self {
            zone: parse_enum(params, "zone")?,
            include_library: parse_bool(params, "include_library"),
        })
    }
}

impl WebQuery for GetSessionEditDetail {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { session_id: require_param(params, "session_id")? })
    }
}

impl WebQuery for GetIntakeConfirmation {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self {
            source: parse_enum(params, "source")?,
            zone: parse_optional_enum(params, "zone")?,
        })
    }
}

#[allow(deprecated)]
impl WebQuery for GetTagCanonicitySignalData {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self {
            signal_key: require_param(params, "signal_key")?,
            kind: parse_enum(params, "kind")?,
        })
    }
}

impl WebQuery for GetTagEditorFiles {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self {
            rel_path: require_param(params, "rel_path")?.into(),
            mode: parse_enum(params, "mode")?,
        })
    }
}

impl WebQuery for GetDirectoryListing {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self {
            zone: parse_enum(params, "zone")?,
            parent: params.get("parent").cloned(),
        })
    }
}

impl WebQuery for SearchCorpusFiles {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self {
            query: require_param(params, "query")?,
            limit: params.get("limit").and_then(|v| v.parse().ok()).unwrap_or(200),
        })
    }
}

impl WebQuery for GetTagCanonicityResolution {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self {
            tag_name: require_param(params, "tag_name")?,
            zone: parse_enum(params, "zone")?,
            filter_existing_canonicals: parse_bool(params, "filter_existing_canonicals"),
        })
    }
}

impl WebQuery for GetCompoundSplitResolution {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self {
            tag_name: require_param(params, "tag_name")?,
            zone: parse_enum(params, "zone")?,
            safe_only: parse_bool(params, "safe_only"),
        })
    }
}

impl WebQuery for GetAcoustidMatches {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { confidence: parse_enum(params, "confidence")? })
    }
}

impl WebQuery for GetReleaseReview {
    fn from_web(params: &HashMap<String, String>, _: &[u8]) -> Result<Self, ApiError> {
        Ok(Self { filter: parse_enum(params, "filter")? })
    }
}

// ============================================================================
// Route dispatch
// ============================================================================

/// Parse kebab-case path segment via `DomainQueryRoute` (typed, from mm-meta),
/// then exhaustive-match to build the `DomainQueryPayload`. Adding a query to
/// `domain_query_protocol!` without handling it here is a compile error.
#[allow(deprecated)]
fn build_domain_payload(
    name: &str,
    params: &HashMap<String, String>,
    body: &[u8],
) -> Result<DomainQueryPayload, ApiError> {
    let route = DomainQueryRoute::from_route_name(name)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown query: '{name}'")))?;

    dispatch_route!(route, params, body;
        GetInsights, GetInboxOverview, GetDeployStatus, GetEditHistory,
        GetExternalMatches, GetPackingDirs,
        GetOobSyncFiles, GetOobFilesBucketed, GetOobConflictByBucket,
        GetMovedFiles, GetMissingAlbumSingleSignals, GetEditHistoryExport,
        GetCompoundSignalGroups, GetPackingKnots, GetPackingInodePaths,
        GetInconsistentAlbumArtistKeys, GetTagCanonicityKeys, GetDiscExtractionData,
        GetMissingFileData, GetMissingDirectoryData, GetCorruptFileData,
        GetSubparDuplicateData, GetDirectoryClusterData, GetReleaseOverlapData,
        GetShitFormatData, GetInboxCorpusMatchData, GetDeployData,
        GetManualReviewData, GetCorpusTags, GetPackingBrowserData,
        GetUnsolvedPackingData,
        GetAudioFilesByInodes, GetMissingTagAudioFiles, GetAllAudioFilesWithTags,
        GetSessionEditDetail, GetCurrentTagValues,
        GetIntakeConfirmation, GetCompoundSplitGroupData,
        GetTagCanonicitySignalData, GetRecordingBatchData, GetReleaseStagingData,
        GetTagEditorFiles, GetInboxOrganizeData, GetFileTagValues,
        GetDirectoryListing, SearchCorpusFiles, SearchWithConditions,
        GetTagCanonicityResolution, GetCompoundSplitResolution,
        GetAcoustidMatches, GetReleaseReview,
    )
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

fn parse_bool(params: &HashMap<String, String>, key: &str) -> bool {
    params.get(key).map(|v| v == "true").unwrap_or(false)
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
