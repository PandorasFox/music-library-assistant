//! Domain query struct definitions and protocol bridge types.
//!
//! Contains all 47 domain query struct definitions (data only — no `DomainQuery`
//! impls, those stay in mm where `ReadOnlyDb` lives), plus the `DomainQueryPayload`
//! and `DomainQueryResult` protocol enums and `ProtocolQuery` impls.

use serde::{Deserialize, Serialize};

// ============================================================================
// Wire types for response types not yet moved to mm-meta view modules
// ============================================================================

// TODO: move these to crate::views once the UI disc_extraction_modal types are
// extracted from mm into mm-meta.

/// Per-file entry within a disc extraction group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscFileEntry {
    pub inode: i64,
    pub path: String,
    /// What the source tag currently says (e.g., "Some Album, Disc 2" or "A01").
    pub original_value: String,
    /// What the source tag will become after extraction (e.g., "Some Album" or "01").
    pub cleaned_value: String,
    /// Source tag name ("ALBUM" or "TRACKNUMBER").
    pub source_tag: String,
}

/// A single group for disc extraction resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscExtractionGroup {
    /// Description of what's being extracted (for info bar).
    pub description: String,
    /// Disc value to write.
    pub disc_value: String,
    /// Per-file entries.
    pub files: Vec<DiscFileEntry>,
}

/// Loaded data for the disc extraction modal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscExtractionModalData {
    pub groups: Vec<DiscExtractionGroup>,
}

/// Convert letter prefix to disc number: A→1, B→2, etc.
///
/// Used by disc extraction query when `map_letters_to_numbers` is set.
pub fn letter_to_number(prefix: &str) -> String {
    if prefix.len() == 1 {
        let ch = prefix.chars().next().unwrap().to_ascii_uppercase();
        if ch.is_ascii_uppercase() {
            return ((ch as u32 - 'A' as u32) + 1).to_string();
        }
    }
    // Multi-letter or non-alpha: return as-is
    prefix.to_string()
}

// TODO: move to crate::signals::data once signal wrapper types are extracted.

/// Wire-safe projection of MissingAlbumSingleSignal (key + data).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingAlbumSingleSignalWire {
    pub key: String,
    pub data: crate::signals::data::MissingAlbumSingleData,
}

// ============================================================================
// Summary Queries (unit structs, cached at protocol level)
// ============================================================================

/// Corpus health insights: file state, tag squash, and other signal counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetInsights;

/// Inbox file counts by category (unindexed, corpus match, organizable, etc.).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetInboxOverview;

/// Current deploy status: library health and per-library file counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetDeployStatus;

/// Edit session list with timestamps and edit counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetEditHistory;

/// External match data: confidence buckets, packing counts, untagged entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetExternalMatches;

/// Packing directory data: assigned file paths and directory categories.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetPackingDirs;

// ============================================================================
// Detail Queries (Wave 1: simple return types, no inode resolution)
// ============================================================================

/// OOB sync files (purely one-direction tag mismatches).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetOobSyncFiles;

/// OOB files classified into conflict buckets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetOobFilesBucketed;

/// Files with moved-file signals (same inode, different path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMovedFiles;

/// Tracks missing an ALBUM tag but having ARTIST and TITLE (album-less singles).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMissingAlbumSingleSignals;

/// Edit history rows for export. `None` = all sessions; `Some(id)` = single session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetEditHistoryExport {
    pub session_id: Option<String>,
}

/// Compound tag signal groups (for compound split resolution).
/// `Zone::Corpus` uses safety/tag filtering; `Zone::Inbox` returns all inbox groups.
#[deprecated(note = "use GetCompoundSplitResolution")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCompoundSignalGroups {
    pub zone: crate::db_types::Zone,
    pub safe_only: bool,
    pub tag_filter: Option<String>,
}

/// Packing knot data (conflict tangles requiring review).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPackingKnots;

/// Packing inode-to-path mapping for knot browser display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPackingInodePaths;

// ============================================================================
// Detail Queries (Wave 2: signal key queries for canonicity resolution)
// ============================================================================

/// Aggregate signal keys for InconsistentAlbumArtist signals.
#[deprecated(note = "use GetTagCanonicityResolution")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInconsistentAlbumArtistKeys;

/// Aggregate signal keys for tag canonicity signals, optionally filtered by tag prefix.
/// `Zone::Corpus` queries `TagCanonicitySignal`; `Zone::Inbox` queries `InboxTagCanonicitySignal`.
#[deprecated(note = "use GetTagCanonicityResolution")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTagCanonicityKeys {
    pub zone: crate::db_types::Zone,
    pub tag_filter: Option<String>,
}

/// Disc extraction signals resolved into modal-ready data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetDiscExtractionData {
    pub map_letters_to_numbers: bool,
}

// ============================================================================
// Detail Queries (Wave 3: modal init loaders)
// ============================================================================

/// Missing file data: restorable and non-restorable missing corpus files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMissingFileData;

/// Missing directory data: directories no longer present on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMissingDirectoryData;

/// Corrupt file data: files that failed indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCorruptFileData;

/// Subpar duplicate data: lower-quality versions of existing files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetSubparDuplicateData;

/// Cross-source directory overlap clusters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetDirectoryClusterData;

/// Release overlap clusters (reuses directory cluster modal data).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetReleaseOverlapData;

/// Shit format file data: non-Vorbis containers needing remux/transcode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetShitFormatData;

/// Inbox corpus match data with configurable bitrate fuzz tolerance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInboxCorpusMatchData {
    pub bitrate_fuzz_percent: f64,
}

/// Deploy modal data with optional config for library assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetDeployData {
    pub config: Option<crate::config::Config>,
}

/// Manual review data for a specific review kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetManualReviewData {
    pub kind: crate::views::review_match::ReviewKind,
}

/// Corpus tags for a single inode (for tag editor fill-from-DB).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCorpusTags {
    pub inode: i64,
}

/// Packed releases by category with all packing signal data for the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPackingBrowserData {
    pub category_prefix: String,
}

/// Unmatched corpus tracks filtered by unsolved category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetUnsolvedPackingData {
    pub category: String,
}

// ============================================================================
// Detail Queries (Wave 4: composite queries collapsed into single execute)
// ============================================================================

/// Audio files by inodes for a specific zone (for tag editor launch).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAudioFilesByInodes {
    pub inodes: Vec<i64>,
    pub zone: crate::db_types::Zone,
}

/// Missing tag resolution: collect unique inodes from MissingTag signals, return audio files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMissingTagAudioFiles;

/// All audio files with tags for a zone (for tag search).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAllAudioFilesWithTags {
    pub zone: crate::db_types::Zone,
    pub include_library: bool,
}

/// Session edit detail: edits + resolved inode paths (for history expansion).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetSessionEditDetail {
    pub session_id: String,
}

/// Resolve current tag values for a list of (inode, field_name) pairs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCurrentTagValues {
    pub queries: Vec<(i64, String)>,
}

// ============================================================================
// Wave 5: Remaining closure conversions
// ============================================================================

/// Gather unindexed files for intake confirmation.
///
/// `zone: None` checks both corpus and inbox (startup mode).
/// `zone: Some(Zone::Corpus)` or `Some(Zone::Inbox)` checks one zone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetIntakeConfirmation {
    pub source: crate::views::startup_organize::IntakeSource,
    pub zone: Option<crate::db_types::Zone>,
}

/// Load compound split modal data for a specific compound group.
#[deprecated(note = "use GetCompoundSplitResolution")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCompoundSplitGroupData {
    pub group: crate::signals::data::CompoundGroup,
    pub zone: crate::db_types::Zone,
}

/// Load tag canonicity signal data for a specific signal key.
#[deprecated(note = "use GetTagCanonicityResolution")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTagCanonicitySignalData {
    pub signal_key: String,
    pub kind: crate::views::canonicity_compound::CanonicitySignalKind,
}

/// Batch-load MB recording summaries and detail data from cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRecordingBatchData {
    pub recording_ids: Vec<String>,
    pub preferred_locales: Vec<String>,
}

/// Load MB cache bundle and current tags for release approval staging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetReleaseStagingData {
    pub release_ids: Vec<String>,
    pub recording_ids: Vec<String>,
    pub inodes: Vec<i64>,
}

/// Load audio files for the tag editor.
///
/// In `Directory` mode, loads all files recursively under `rel_path`.
/// In `SingleFile` mode, loads direct siblings in the parent dir and
/// returns the index of the target file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTagEditorFiles {
    pub rel_path: std::path::PathBuf,
    pub mode: crate::domain_query_types::TagEditorLoadMode,
}

/// Load organizable inbox directories for the organize workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInboxOrganizeData {
    pub config: crate::config::Config,
}

/// Read tags from disk for a batch of audio files (by inode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetFileTagValues {
    pub inodes: Vec<i64>,
    pub zone: crate::db_types::Zone,
}

/// Packed tag canonicity resolution: all clusters for a tag+zone in one response.
/// Replaces the two-phase GetTagCanonicityKeys + GetTagCanonicitySignalData pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTagCanonicityResolution {
    pub tag_name: String,
    pub zone: crate::db_types::Zone,
}

/// Packed compound split resolution: all groups for a tag+zone in one response.
/// Replaces the two-phase GetCompoundSignalGroups + GetCompoundSplitGroupData pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCompoundSplitResolution {
    pub tag_name: String,
    pub zone: crate::db_types::Zone,
    pub safe_only: bool,
}

// ============================================================================
// Web File Browser & Search
// ============================================================================

/// List immediate children of a directory: subdirectories with file counts,
/// and audio files with display metadata. No tags loaded.
///
/// `parent: None` = list top-level source directories.
/// `parent: Some("path/to/dir")` = children of that directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetDirectoryListing {
    pub zone: crate::db_types::Zone,
    pub parent: Option<String>,
}

/// Server-side substring search across paths and tag values, with result cap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCorpusFiles {
    pub query: String,
    pub limit: usize,
}

/// Server-side structured search: translates typed conditions to SQL.
///
/// Replaces client-side `evaluate_conditions()` + `GetAllAudioFilesWithTags` pattern.
/// Each condition maps to SQL predicates composed with the condition's logical operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchWithConditions {
    pub conditions: Vec<crate::domain_query_types::SearchConditionWire>,
    pub zone: crate::db_types::Zone,
    pub limit: usize,
}

// ============================================================================
// Protocol Bridge Macro
// ============================================================================

/// Generates protocol-level types for domain queries:
/// - `DomainQueryPayload` enum (one variant per query, carrying the query struct)
/// - `DomainQueryResult` enum (one variant per query, carrying the response type)
/// - `impl ProtocolQuery for Q` for each query (typed send/receive via protocol)
///
/// Does NOT generate `dispatch_domain_query()` or `DomainQuery` impls — those
/// stay in mm where `ReadOnlyDb` lives.
macro_rules! domain_query_protocol {
    ( $( $query:ident => $response:ty ),+ $(,)? ) => {
        /// Wire enum carrying a domain query payload.
        /// One variant per registered domain query type.
        #[derive(Debug, Clone, Serialize, Deserialize)]
        pub enum DomainQueryPayload {
            $( $query($query), )+
        }

        /// Wire enum carrying a domain query result.
        /// One variant per registered domain query type.
        #[derive(Debug, Clone, Serialize, Deserialize)]
        pub enum DomainQueryResult {
            $( $query($response), )+
        }

        $(
            impl crate::protocol::ProtocolQuery for $query {
                type Response = $response;

                fn into_payload(self) -> crate::protocol::QueryPayload {
                    crate::protocol::QueryPayload::Domain(Box::new(DomainQueryPayload::$query(self)))
                }

                fn extract_response(
                    resp: crate::protocol::QueryResponse,
                ) -> Self::Response {
                    match resp {
                        crate::protocol::QueryResponse::Domain(
                            DomainQueryResult::$query(r),
                        ) => r,
                        _ => unreachable!("protocol bug: expected {} response", stringify!($query)),
                    }
                }
            }
        )+
    };
}

domain_query_protocol! {
    // Summary queries
    GetInsights => crate::views::InsightsData,
    GetInboxOverview => crate::views::InboxOverviewData,
    GetDeployStatus => crate::views::DeployStatus,
    GetEditHistory => crate::views::EditHistoryData,
    GetExternalMatches => crate::views::ExternalMatchesData,
    GetPackingDirs => crate::domain_query_types::PackingDirsData,

    // Detail queries (Wave 1)
    GetOobSyncFiles => Vec<crate::views::OobSyncFile>,
    GetOobFilesBucketed => Vec<crate::views::BucketedOobFile>,
    GetMovedFiles => Vec<crate::views::MovedFileInfo>,
    GetMissingAlbumSingleSignals => Vec<MissingAlbumSingleSignalWire>,
    GetEditHistoryExport => Vec<crate::views::EditHistoryExportRow>,
    GetCompoundSignalGroups => Vec<crate::signals::data::CompoundGroup>,
    GetPackingKnots => Vec<crate::signals::data::PackingKnotData>,
    GetPackingInodePaths => Vec<(i64, String)>,

    // Detail queries (Wave 2)
    GetInconsistentAlbumArtistKeys => Vec<String>,
    GetTagCanonicityKeys => Vec<String>,
    GetDiscExtractionData => DiscExtractionModalData,

    // Detail queries (Wave 3: modal init loaders)
    GetMissingFileData => crate::views::health_modals::MissingFileModalData,
    GetMissingDirectoryData => crate::views::health_modals::MissingDirectoryModalData,
    GetCorruptFileData => crate::views::health_modals::CorruptFileModalData,
    GetSubparDuplicateData => crate::views::health_modals::SubparDuplicateModalData,
    GetDirectoryClusterData => crate::views::cluster_deploy::DirectoryClusterModalData,
    GetReleaseOverlapData => crate::views::cluster_deploy::DirectoryClusterModalData,
    GetShitFormatData => crate::views::cluster_deploy::ShitFormatModalData,
    GetInboxCorpusMatchData => crate::views::review_match::InboxCorpusMatchModalData,
    GetDeployData => crate::views::cluster_deploy::DeployModalData,
    GetManualReviewData => crate::views::review_match::ManualReviewData,
    GetCorpusTags => Vec<(String, String)>,
    GetPackingBrowserData => crate::domain_query_types::PackingBrowserData,
    GetUnsolvedPackingData => Vec<(i64, String, crate::signals::data::UnmatchedCorpusTrackData)>,

    // Detail queries (Wave 4: composite)
    GetAudioFilesByInodes => Vec<crate::db_types::AudioFile>,
    GetMissingTagAudioFiles => Vec<crate::db_types::AudioFile>,
    GetAllAudioFilesWithTags => Vec<crate::domain_query_types::AudioFileWithTags>,
    GetSessionEditDetail => crate::domain_query_types::SessionEditDetail,
    GetCurrentTagValues => Vec<Option<String>>,

    // Wave 5
    GetIntakeConfirmation => Option<crate::views::startup_organize::IntakeConfirmationState>,
    GetCompoundSplitGroupData => Option<crate::views::canonicity_compound::CompoundSplitDataV2>,
    GetTagCanonicitySignalData => Option<crate::views::canonicity_compound::TagCanonicalityModalDataV2>,
    GetRecordingBatchData => crate::domain_query_types::RecordingBatchResult,
    GetReleaseStagingData => crate::domain_query_types::ReleaseStagingData,
    GetTagEditorFiles => (Vec<crate::db_types::AudioFile>, usize),
    GetInboxOrganizeData => Vec<crate::views::startup_organize::InboxDirectory>,
    GetFileTagValues => Vec<(i64, Vec<(String, String)>)>,

    // Web file browser & search
    GetDirectoryListing => Vec<crate::domain_query_types::DirectoryListingEntry>,
    SearchCorpusFiles => Vec<crate::domain_query_types::SearchResult>,
    SearchWithConditions => Vec<crate::domain_query_types::SearchResult>,

    // Packed resolution queries (cluster-nav)
    GetTagCanonicityResolution => crate::views::canonicity_compound::TagCanonicityResolutionData,
    GetCompoundSplitResolution => crate::views::canonicity_compound::CompoundSplitResolutionData,
}
