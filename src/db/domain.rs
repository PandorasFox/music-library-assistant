//! Domain Query Layer
//!
//! Typed query vocabulary for all read access to MM's database. Any client
//! (TUI, web API, future tooling) communicates with the Witch through these
//! query types rather than raw `ReadOnlyDb` handles.
//!
//! ## Trait Contract
//!
//! Every query implements `DomainQuery` with exactly this shape:
//! - All inputs are fields on the query struct
//! - All outputs are in the associated `Response` type
//! - `execute` takes `self` by value and `&ReadOnlyDb` — nothing else
//! - Response types are `Serialize` (web-ready from day one)
//! - Errors are handled internally (return usable defaults, never `Result`)
//!
//! ## Macro
//!
//! The `define_domain_query!` macro generates the query struct, `DomainQuery` impl,
//! and optional `CachedQuery` impl from a compact declaration. Forms:
//!
//! ```ignore
//! // Simple: unit struct, single db method call, unwrap_or_default
//! define_domain_query! {
//!     /// Doc comment
//!     GetFoo => FooData, cached(15), db.get_foo_data()
//! }
//!
//! // Body: unit struct, custom execute logic with `db` in scope
//! define_domain_query! {
//!     /// Doc comment
//!     GetBar => BarData, cached(30), |db| {
//!         let x = db.get_x().unwrap_or_default();
//!         let y = db.get_y().unwrap_or_default();
//!         BarData { x, y }
//!     }
//! }
//!
//! // Uncached (detail query):
//! define_domain_query! {
//!     /// Doc comment
//!     GetBaz => BazData, uncached, db.get_baz_data()
//! }
//!
//! // Modal load shorthand: unit struct, Response::load(db).ok().unwrap_or_default()
//! define_domain_query! {
//!     /// Doc comment
//!     GetQux => QuxModalData, uncached, modal_load
//! }
//!
//! // Parameterized: struct with fields, custom execute body (s = &self, db = &ReadOnlyDb)
//! define_domain_query! {
//!     /// Doc comment
//!     GetQuux { field1: Type1, field2: Type2 } => QuuxData, uncached, |s, db| {
//!         db.some_query(&s.field1, s.field2).unwrap_or_default()
//!     }
//! }
//! ```
//!
//! See `docs/DOMAIN_QUERY_STRATEGY.md` for the full design rationale.

use std::time::Duration;

use serde::Serialize;

use crate::db::ReadOnlyDb;

// ============================================================================
// Core Traits
// ============================================================================

/// Every domain query implements this. The trait is the contract
/// that the `define_domain_query!` macro generates against.
///
/// Implementors must follow the uniformity rules:
/// 1. All inputs are fields on `self`
/// 2. All outputs are in `Response`
/// 3. No side channels, no `&mut`, no extra context parameters
/// 4. Errors handled internally — return defaults, never `Result`
pub trait DomainQuery: Send + 'static {
    /// The response type. Must be `Serialize` for web transport readiness.
    type Response: Serialize + Send + 'static;

    /// Execute the query against a read-only database connection.
    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response;
}

/// Summary queries that benefit from throttled caching.
/// Detail queries implement only `DomainQuery`.
pub trait CachedQuery: DomainQuery {
    /// How long cached results remain fresh before re-query.
    const THROTTLE: Duration;
}

// ============================================================================
// Macro
// ============================================================================

/// Generates a domain query struct with `DomainQuery` and optional `CachedQuery` impls.
///
/// See module docs for usage examples.
macro_rules! define_domain_query {
    // Simple form: unit struct, single db method, unwrap_or_default
    (
        $( #[doc = $doc:expr] )*
        $name:ident => $response:ty, cached($secs:expr), db.$method:ident()
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name;

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
                db.$method().unwrap_or_default()
            }
        }

        impl CachedQuery for $name {
            const THROTTLE: Duration = Duration::from_secs($secs);
        }
    };

    // Body form: unit struct, custom execute expression with db closure
    (
        $( #[doc = $doc:expr] )*
        $name:ident => $response:ty, cached($secs:expr), |$db:ident| $body:block
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name;

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, $db: &ReadOnlyDb<'_>) -> Self::Response {
                $body
            }
        }

        impl CachedQuery for $name {
            const THROTTLE: Duration = Duration::from_secs($secs);
        }
    };

    // Simple form, uncached (detail query)
    (
        $( #[doc = $doc:expr] )*
        $name:ident => $response:ty, uncached, db.$method:ident()
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name;

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
                db.$method().unwrap_or_default()
            }
        }
    };

    // Body form, uncached (detail query)
    (
        $( #[doc = $doc:expr] )*
        $name:ident => $response:ty, uncached, |$db:ident| $body:block
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name;

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, $db: &ReadOnlyDb<'_>) -> Self::Response {
                $body
            }
        }
    };

    // Modal load shorthand: unit struct, ModalType::load(db).ok().unwrap_or_default()
    (
        $( #[doc = $doc:expr] )*
        $name:ident => $response:ty, uncached, modal_load
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name;

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
                <$response>::load(db).ok().unwrap_or_default()
            }
        }
    };

    // Parameterized body form, uncached: struct with fields + custom execute body
    (
        $( #[doc = $doc:expr] )*
        $name:ident { $( $field:ident : $ftype:ty ),+ $(,)? } => $response:ty, uncached, |$s:ident, $db:ident| $body:block
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name {
            $( pub $field: $ftype ),+
        }

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, $db: &ReadOnlyDb<'_>) -> Self::Response {
                let $s = &self;
                $body
            }
        }
    };

    // Parameterized body form, cached: struct with fields + custom execute body
    (
        $( #[doc = $doc:expr] )*
        $name:ident { $( $field:ident : $ftype:ty ),+ $(,)? } => $response:ty, cached($secs:expr), |$s:ident, $db:ident| $body:block
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name {
            $( pub $field: $ftype ),+
        }

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, $db: &ReadOnlyDb<'_>) -> Self::Response {
                let $s = &self;
                $body
            }
        }

        impl CachedQuery for $name {
            const THROTTLE: Duration = Duration::from_secs($secs);
        }
    };
}

// ============================================================================
// Summary Queries (CacheReady variants)
// ============================================================================

use crate::meta::signals::data::{CompoundGroup, MissingAlbumSingleSignal};
use crate::meta::views::{
    BucketedOobFile, DeployStatus, EditHistoryData, EditHistoryExportRow, ExternalMatchesData,
    InboxOverviewData, InsightsData, MovedFileInfo, OobSyncFile,
};
use crate::witch::cache_thread::PackingDirsData;

define_domain_query! {
    /// Corpus health insights: file state, tag squash, and other signal counts.
    GetInsights => InsightsData, cached(30), db.get_insights_data()
}

define_domain_query! {
    /// Inbox file counts by category (unindexed, corpus match, organizable, etc.).
    GetInboxOverview => InboxOverviewData, cached(15), db.get_inbox_overview_data()
}

define_domain_query! {
    /// Current deploy status: library health and per-library file counts.
    GetDeployStatus => DeployStatus, cached(15), db.get_deploy_status()
}

define_domain_query! {
    /// Edit session list with timestamps and edit counts.
    GetEditHistory => EditHistoryData, cached(30), |db| {
        let sessions = db.get_edit_sessions().unwrap_or_default();
        EditHistoryData { sessions }
    }
}

define_domain_query! {
    /// External match data: confidence buckets, packing counts, untagged entries.
    GetExternalMatches => ExternalMatchesData, cached(15), db.get_external_matches_data()
}

define_domain_query! {
    /// Packing directory data: assigned file paths and directory categories.
    GetPackingDirs => PackingDirsData, cached(30), |db| {
        let file_paths = db.get_packing_assigned_paths().unwrap_or_default();
        let dir_categories = db.get_packing_directory_categories().unwrap_or_default();
        PackingDirsData { file_paths, dir_categories }
    }
}

// ============================================================================
// Detail Queries (Wave 1: simple return types, no inode resolution)
// ============================================================================

define_domain_query! {
    /// OOB sync files (purely one-direction tag mismatches).
    GetOobSyncFiles => Vec<OobSyncFile>, uncached, db.get_oob_sync_files()
}

define_domain_query! {
    /// OOB files classified into conflict buckets.
    GetOobFilesBucketed => Vec<BucketedOobFile>, uncached, db.get_oob_files_bucketed()
}

define_domain_query! {
    /// Files with moved-file signals (same inode, different path).
    GetMovedFiles => Vec<MovedFileInfo>, uncached, db.get_moved_files()
}

define_domain_query! {
    /// Missing album single signals (tracks without ALBUM but with ARTIST+TITLE).
    GetMissingAlbumSingleSignals => Vec<MissingAlbumSingleSignal>, uncached, db.get_missing_album_single_signals()
}

define_domain_query! {
    /// Full edit history for a specific session (for export).
    GetSessionEditHistory { session_id: String } => Vec<EditHistoryExportRow>, uncached, |s, db| {
        db.get_session_edit_history(&s.session_id)
            .unwrap_or_default()
    }
}

define_domain_query! {
    /// Full edit history across all sessions (for export).
    GetAllEditHistory => Vec<EditHistoryExportRow>, uncached, db.get_all_edit_history()
}

define_domain_query! {
    /// Compound tag signal groups (for compound split resolution).
    GetCompoundSignalGroups { safe_only: bool, tag_filter: Option<String> } => Vec<CompoundGroup>, uncached, |s, db| {
        db.get_compound_signal_groups_by_safety(s.safe_only, s.tag_filter.as_deref())
            .unwrap_or_default()
    }
}

define_domain_query! {
    /// Inbox compound tag signal groups.
    GetInboxCompoundSignalGroups => Vec<CompoundGroup>, uncached, db.get_inbox_compound_signal_groups()
}

define_domain_query! {
    /// Packing knot data (conflict tangles requiring review).
    GetPackingKnots => Vec<crate::meta::signals::data::PackingKnotData>, uncached, db.get_packing_knots()
}

define_domain_query! {
    /// Packing inode-to-path mapping for knot browser display.
    GetPackingInodePaths => Vec<(i64, String)>, uncached, db.get_packing_inode_paths()
}

// ============================================================================
// Detail Queries (Wave 2: signal key queries for canonicity resolution)
// ============================================================================

define_domain_query! {
    /// Aggregate signal keys for InconsistentAlbumArtist signals.
    GetInconsistentAlbumArtistKeys => Vec<String>, uncached, |db| {
        use crate::meta::signals::data::InconsistentAlbumArtistSignal;
        db.aggregate_signal_keys::<InconsistentAlbumArtistSignal>().unwrap_or_default()
    }
}

define_domain_query! {
    /// Aggregate signal keys for TagCanonicity signals, optionally filtered by tag prefix.
    GetTagCanonicityKeys { tag_filter: Option<String> } => Vec<String>, uncached, |s, db| {
        use crate::meta::signals::data::TagCanonicitySignal;
        let all_keys = db
            .aggregate_signal_keys::<TagCanonicitySignal>()
            .unwrap_or_default();
        match &s.tag_filter {
            Some(tag_name) => {
                let prefix = format!("{}:", tag_name);
                all_keys
                    .into_iter()
                    .filter(|k| k.starts_with(&prefix))
                    .collect()
            }
            None => all_keys,
        }
    }
}

define_domain_query! {
    /// Aggregate signal keys for InboxTagCanonicity signals.
    GetInboxTagCanonicityKeys => Vec<String>, uncached, |db| {
        use crate::meta::signals::data::InboxTagCanonicitySignal;
        db.aggregate_signal_keys::<InboxTagCanonicitySignal>().unwrap_or_default()
    }
}

define_domain_query! {
    /// Disc extraction signals resolved into modal-ready data.
    ///
    /// Loads signals, resolves file paths, and applies config (letter→number mapping)
    /// to produce the full DiscExtractionData the modal needs.
    GetDiscExtractionData { map_letters_to_numbers: bool } =>
        crate::ui::disc_extraction_modal::DiscExtractionData, uncached, |s, db| {
        use crate::ui::disc_extraction_modal::DiscExtractionData;

        let signals = db.get_disc_extraction_signals().unwrap_or_default();
        if signals.is_empty() {
            return DiscExtractionData { groups: Vec::new() };
        }

        let all_inodes: Vec<i64> = signals
            .iter()
            .flat_map(|sig| sig.data.inodes.iter().copied())
            .collect();
        let path_map = db
            .get_file_paths_batch(crate::db::types::Zone::Corpus, &all_inodes)
            .unwrap_or_default();

        DiscExtractionData::from_signals(
            signals,
            |inode| path_map.get(&inode).cloned().unwrap_or_else(|| format!("<inode {}>", inode)),
            s.map_letters_to_numbers,
        )
    }
}

// ============================================================================
// Detail Queries (Wave 3: modal init loaders)
// ============================================================================

use crate::ui::corrupt_file_modal;
use crate::ui::deploy_modal;
use crate::ui::directory_cluster_modal;
use crate::ui::inbox_corpus_match_modal;
use crate::ui::manual_review_modal;
use crate::ui::missing_directory_modal;
use crate::ui::missing_file_modal;
use crate::ui::shit_format_modal;
use crate::ui::subpar_duplicate_modal;

define_domain_query! {
    /// Missing file data: restorable and non-restorable missing corpus files.
    GetMissingFileData => missing_file_modal::MissingFileModalData, uncached, modal_load
}

define_domain_query! {
    /// Missing directory data: directories no longer present on disk.
    GetMissingDirectoryData => missing_directory_modal::MissingDirectoryModalData, uncached, modal_load
}

define_domain_query! {
    /// Corrupt file data: files that failed indexing.
    GetCorruptFileData => corrupt_file_modal::CorruptFileModalData, uncached, modal_load
}

define_domain_query! {
    /// Subpar duplicate data: lower-quality versions of existing files.
    GetSubparDuplicateData => subpar_duplicate_modal::SubparDuplicateModalData, uncached, modal_load
}

define_domain_query! {
    /// Cross-source directory overlap clusters.
    GetDirectoryClusterData => directory_cluster_modal::DirectoryClusterModalData, uncached, modal_load
}

define_domain_query! {
    /// Release overlap clusters (reuses directory cluster modal data).
    GetReleaseOverlapData => directory_cluster_modal::DirectoryClusterModalData, uncached, |db| {
        directory_cluster_modal::DirectoryClusterModalData::load_release_overlaps(db).ok().unwrap_or_default()
    }
}

define_domain_query! {
    /// Shit format file data: non-Vorbis containers needing remux/transcode.
    GetShitFormatData => shit_format_modal::ShitFormatModalData, uncached, modal_load
}

define_domain_query! {
    /// Inbox corpus match data with configurable bitrate fuzz tolerance.
    GetInboxCorpusMatchData { bitrate_fuzz_percent: f64 } => inbox_corpus_match_modal::InboxCorpusMatchModalData, uncached, |s, db| {
        inbox_corpus_match_modal::InboxCorpusMatchModalData::load(db, s.bitrate_fuzz_percent)
            .ok()
            .unwrap_or_default()
    }
}

define_domain_query! {
    /// Deploy modal data with optional config for library assignment.
    GetDeployData { config: Option<crate::config::Config> } => deploy_modal::DeployModalData, uncached, |s, db| {
        deploy_modal::DeployModalData::load(db, s.config.as_ref())
            .unwrap_or_default()
    }
}

define_domain_query! {
    /// Manual review data for a specific review kind.
    GetManualReviewData { kind: manual_review_modal::types::ReviewKind } => manual_review_modal::types::ManualReviewData, uncached, |s, db| {
        manual_review_modal::types::ManualReviewData::load(db, s.kind)
            .ok()
            .unwrap_or_default()
    }
}

define_domain_query! {
    /// Corpus tags for a single inode (for tag editor fill-from-DB).
    GetCorpusTags { inode: i64 } => Vec<(String, String)>, uncached, |s, db| {
        db.get_tags::<crate::zones::CorpusZone>(s.inode)
            .unwrap_or_default()
            .into_iter()
            .map(|t| (t.tag_name, t.tag_value))
            .collect()
    }
}

/// All data needed to build a release packing browser view.
#[derive(serde::Serialize)]
pub struct PackingBrowserData {
    pub packed: Vec<crate::meta::signals::data::PackedReleaseData>,
    pub packing: Vec<(i64, String, crate::meta::signals::data::ReleasePackingData)>,
    pub unfilled: Vec<crate::meta::signals::data::UnfilledReleaseSlotData>,
    pub alternatives: Vec<crate::meta::signals::data::AlternativeReleasePackingData>,
    pub va_overrides: Vec<crate::meta::signals::data::VariousArtistsOverrideData>,
}

define_domain_query! {
    /// Packed releases by category with all packing signal data for the browser.
    GetPackingBrowserData { category_prefix: String } => PackingBrowserData, uncached, |s, db| {
        PackingBrowserData {
            packed: db.get_packed_releases_by_category(&s.category_prefix).unwrap_or_default(),
            packing: db.get_release_packing_signal_data().unwrap_or_default(),
            unfilled: db.get_unfilled_release_slot_signal_data().unwrap_or_default(),
            alternatives: db.get_alternative_release_packing_data().unwrap_or_default(),
            va_overrides: db.get_various_artists_override_data().unwrap_or_default(),
        }
    }
}

define_domain_query! {
    /// Unmatched corpus tracks filtered by unsolved category.
    GetUnsolvedPackingData { category: String } => Vec<(i64, String, crate::meta::signals::data::UnmatchedCorpusTrackData)>, uncached, |s, db| {
        db.get_unmatched_corpus_track_signal_data_by_category(&s.category)
            .unwrap_or_default()
    }
}

// ============================================================================
// Detail Queries (Wave 4: composite queries collapsed into single execute)
// ============================================================================

define_domain_query! {
    /// Audio files by inodes for a specific zone (for tag editor launch).
    GetAudioFilesByInodes { inodes: Vec<i64>, zone: crate::db::types::Zone } => Vec<crate::db::types::AudioFile>, uncached, |s, db| {
        db.get_audio_files_by_inodes(&s.inodes, s.zone)
            .unwrap_or_default()
    }
}

define_domain_query! {
    /// Missing tag resolution: collect unique inodes from MissingTag signals, return audio files.
    GetMissingTagAudioFiles => Vec<crate::db::types::AudioFile>, uncached, |db| {
        use std::collections::BTreeSet;
        let signals = db.get_missing_tag_signals().unwrap_or_default();
        let all_inodes: Vec<i64> = signals
            .iter()
            .flat_map(|s| s.data.inodes.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        db.get_audio_files_by_inodes(&all_inodes, crate::db::types::Zone::Corpus)
            .unwrap_or_default()
    }
}

define_domain_query! {
    /// All audio files with tags for a zone (for tag search).
    GetAllAudioFilesWithTags { zone: crate::db::types::Zone, include_library: bool } => Vec<crate::db::queries::files::AudioFileWithTags>, uncached, |s, db| {
        db.get_all_audio_files_with_tags(s.zone, s.include_library)
            .unwrap_or_default()
    }
}

/// Response for session edit detail.
#[derive(serde::Serialize)]
pub struct SessionEditDetail {
    pub edits: Vec<crate::meta::views::EditRecord>,
    pub inode_paths: std::collections::HashMap<i64, String>,
}

define_domain_query! {
    /// Session edit detail: edits + resolved inode paths (for history expansion).
    GetSessionEditDetail { session_id: String } => SessionEditDetail, uncached, |s, db| {
        let edits = db.get_session_edits(&s.session_id).unwrap_or_default();
        let inodes: Vec<i64> = edits.iter().map(|e| e.inode).collect();
        let inode_paths = db
            .get_file_paths_batch(crate::db::types::Zone::Corpus, &inodes)
            .unwrap_or_default();
        SessionEditDetail { edits, inode_paths }
    }
}

define_domain_query! {
    /// Resolve current tag values for a list of (inode, field_name) pairs.
    GetCurrentTagValues { queries: Vec<(i64, String)> } => Vec<Option<String>>, uncached, |s, db| {
        s.queries
            .iter()
            .map(|(inode, field_name)| {
                let tags = db.get_tags::<crate::zones::CorpusZone>(*inode).unwrap_or_default();
                tags.iter()
                    .find(|t| t.tag_name.eq_ignore_ascii_case(field_name))
                    .map(|t| t.tag_value.clone())
            })
            .collect()
    }
}

// ============================================================================
// Wave 5: Remaining closure conversions
// ============================================================================

define_domain_query! {
    /// Gather unindexed files for intake confirmation.
    ///
    /// `zone: None` checks both corpus and inbox (startup mode).
    /// `zone: Some(Zone::Corpus)` or `Some(Zone::Inbox)` checks one zone.
    GetIntakeConfirmation {
        source: crate::ui::startup::IntakeSource,
        zone: Option<crate::db::types::Zone>,
    } => Option<crate::ui::startup::IntakeConfirmationState>, uncached, |s, db| {
        use crate::ui::startup::IntakeConfirmationState;
        match s.zone {
            Some(crate::db::types::Zone::Corpus) => {
                IntakeConfirmationState::gather_zone::<crate::zones::CorpusZone>(db, s.source)
            }
            Some(crate::db::types::Zone::Inbox) => {
                IntakeConfirmationState::gather_zone::<crate::zones::InboxZone>(db, s.source)
            }
            Some(_) => None,
            None => IntakeConfirmationState::gather_startup(db),
        }
    }
}

define_domain_query! {
    /// Load compound split modal data for a specific compound group.
    GetCompoundSplitGroupData {
        group: crate::meta::signals::data::CompoundGroup,
        zone: crate::db::types::Zone,
    } => Option<crate::ui::compound_split_v2::CompoundSplitDataV2>, uncached, |s, db| {
        crate::ui::compound_split_v2::CompoundSplitDataV2::from_compound_group(&s.group, db, s.zone)
    }
}

define_domain_query! {
    /// Load tag canonicity signal data for a specific signal key.
    GetTagCanonicitySignalData {
        signal_key: String,
        kind: crate::ui::CanonicitySignalKind,
    } => Option<crate::ui::tag_canonicity_v2::TagCanonicalityModalDataV2>, uncached, |s, db| {
        load_tag_canonicity_signal_data(&s.signal_key, s.kind, db)
    }
}

/// Load typed tag canonicity signal data by key and kind.
///
/// Extracted from `App::load_typed_signal_data` so it can be called
/// from the domain query without needing `&self`.
fn load_tag_canonicity_signal_data(
    key: &str,
    kind: crate::ui::CanonicitySignalKind,
    read_db: &ReadOnlyDb,
) -> Option<crate::ui::tag_canonicity_v2::TagCanonicalityModalDataV2> {
    use crate::ui::CanonicitySignalKind;
    use crate::ui::tag_canonicity_v2::TagCanonicalityModalDataV2;
    match kind {
        CanonicitySignalKind::TagCanonicity => {
            let signal = read_db.get_tag_canonicity_signal(key).ok()??;
            TagCanonicalityModalDataV2::from_tag_canonicity(&signal, read_db)
        }
        CanonicitySignalKind::InconsistentAlbumArtist => {
            let signal = read_db.get_inconsistent_album_artist_signal(key).ok()??;
            TagCanonicalityModalDataV2::from_inconsistent_album_artist(&signal, read_db)
        }
        CanonicitySignalKind::InboxTagCanonicity => {
            let signal = read_db.get_inbox_tag_canonicity_signal(key).ok()??;
            TagCanonicalityModalDataV2::from_inbox_tag_canonicity(&signal, read_db)
        }
    }
}

/// Response type for batch recording data loading.
#[derive(serde::Serialize)]
pub struct RecordingBatchResult {
    pub summaries: Vec<(String, crate::ui::external_match_modal::types::RecordingSummary)>,
    pub details: Vec<(String, crate::ui::external_match_modal::types::RecordingDetail)>,
}

define_domain_query! {
    /// Batch-load MB recording summaries and detail data from cache.
    GetRecordingBatchData {
        recording_ids: Vec<String>,
        preferred_locales: Vec<String>,
    } => RecordingBatchResult, uncached, |s, db| {
        load_recording_batch_data(&s.recording_ids, &s.preferred_locales, db)
    }
}

/// Batch-load recording data from MB cache.
///
/// Extracted from the inline closure in `start_external_match_review_with`.
fn load_recording_batch_data(
    ids: &[String],
    preferred_locales: &[String],
    db: &ReadOnlyDb,
) -> RecordingBatchResult {
    use crate::external::musicbrainz;
    use crate::ui::external_match_modal::types::{RecordingDetail, RecordingSummary};
    use std::collections::HashSet;

    let mut summaries = Vec::new();
    let mut details = Vec::new();

    for rec_id in ids {
        let rec_cache = db.get_mb_recording_cache(rec_id).ok().flatten();
        let recording =
            rec_cache.and_then(|(json, _)| musicbrainz::parse_recording(&json).ok());

        let Some(rec) = recording else {
            continue;
        };

        // Collect unique artist IDs from credits + relations
        let mut artist_ids: Vec<String> = Vec::new();
        let mut artist_seen = HashSet::new();
        for credit in &rec.artist_credit {
            if artist_seen.insert(credit.artist.id.clone()) {
                artist_ids.push(credit.artist.id.clone());
            }
        }
        for relation in &rec.relations {
            if let Some(ref artist) = relation.artist {
                if artist_seen.insert(artist.id.clone()) {
                    artist_ids.push(artist.id.clone());
                }
            }
        }

        // Load cached artist data
        let artists: Vec<(String, Option<musicbrainz::MbArtist>)> = artist_ids
            .into_iter()
            .map(|id| {
                let parsed = db
                    .get_mb_artist_cache(&id)
                    .ok()
                    .flatten()
                    .and_then(|(json, _)| musicbrainz::parse_artist(&json).ok());
                (id, parsed)
            })
            .collect();

        // Build summary
        let artist_credit = musicbrainz::join_artist_credits_localized(
            &rec.artist_credit,
            &artists,
            preferred_locales,
        );

        summaries.push((
            rec_id.clone(),
            RecordingSummary {
                title: rec.title.clone(),
                artist_credit,
                length_ms: rec.length.map(|l| l as u64),
                release_count: rec.releases.len(),
            },
        ));

        // Load cached release data for full detail
        let releases: Vec<_> = rec
            .releases
            .iter()
            .map(|r| {
                let parsed = db
                    .get_mb_release_cache(&r.id)
                    .ok()
                    .flatten()
                    .and_then(|(json, _)| musicbrainz::parse_release(&json).ok());
                (r.id.clone(), parsed)
            })
            .collect();

        details.push((
            rec_id.clone(),
            RecordingDetail {
                recording: rec,
                artists,
                releases,
            },
        ));
    }

    RecordingBatchResult { summaries, details }
}

/// Response type for release staging data loading.
#[derive(serde::Serialize)]
pub struct ReleaseStagingData {
    pub bundle: crate::external::musicbrainz::MbCacheBundle,
    pub inode_tags: std::collections::HashMap<i64, Vec<(String, String)>>,
}

define_domain_query! {
    /// Load MB cache bundle and current tags for release approval staging.
    GetReleaseStagingData {
        release_ids: Vec<String>,
        recording_ids: Vec<String>,
        inodes: Vec<i64>,
    } => ReleaseStagingData, uncached, |s, db| {
        use crate::external::musicbrainz::MbCacheBundle;
        let bundle = MbCacheBundle::load(db, &s.release_ids, &s.recording_ids);
        let mut tags = std::collections::HashMap::new();
        for inode in &s.inodes {
            tags.insert(
                *inode,
                db.get_tags::<crate::zones::CorpusZone>(*inode)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| (t.tag_name.to_uppercase(), t.tag_value))
                    .collect(),
            );
        }
        ReleaseStagingData { bundle, inode_tags: tags }
    }
}

/// Tag editor file loading mode.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TagEditorLoadMode {
    /// Load all files in directory tree (recursive).
    Directory,
    /// Load siblings in parent directory, select the target file.
    SingleFile,
}

define_domain_query! {
    /// Load audio files for the tag editor.
    ///
    /// In `Directory` mode, loads all files recursively under `rel_path`.
    /// In `SingleFile` mode, loads direct siblings in the parent dir and
    /// returns the index of the target file.
    GetTagEditorFiles {
        rel_path: std::path::PathBuf,
        mode: TagEditorLoadMode,
    } => (Vec<crate::db::types::AudioFile>, usize), uncached, |s, db| {
        load_tag_editor_files(&s.rel_path, &s.mode, db)
    }
}

/// Load audio files for tag editing based on mode.
///
/// Extracted from `App::open_unified_tag_editor` closure logic.
fn load_tag_editor_files(
    rel_path: &std::path::Path,
    mode: &TagEditorLoadMode,
    db: &ReadOnlyDb,
) -> (Vec<crate::db::types::AudioFile>, usize) {
    match mode {
        TagEditorLoadMode::Directory => {
            let files = db
                .get_audio_files_for_tag_editing(rel_path)
                .unwrap_or_default();
            (files, 0)
        }
        TagEditorLoadMode::SingleFile => {
            let rel_parent = match rel_path.parent() {
                Some(p) => p.to_path_buf(),
                None => return (Vec::new(), 0),
            };

            // Load all audio files from parent directory
            let dir_files = db
                .get_audio_files_for_tag_editing(&rel_parent)
                .unwrap_or_default();

            // Filter to only files directly in this directory (not subdirectories)
            let rel_path_str = rel_path.to_string_lossy().to_string();
            let rel_parent_str = rel_parent.to_string_lossy().to_string();
            let files_in_dir: Vec<_> = dir_files
                .into_iter()
                .filter(|f| {
                    if let Some(suffix) = f.path().strip_prefix(&rel_parent_str) {
                        let suffix = suffix.trim_start_matches(std::path::MAIN_SEPARATOR);
                        !suffix.contains(std::path::MAIN_SEPARATOR)
                    } else {
                        false
                    }
                })
                .collect();

            let selected_idx = files_in_dir
                .iter()
                .position(|f| f.path() == rel_path_str)
                .unwrap_or(0);

            if files_in_dir.is_empty() {
                // Fallback: try to get just the single audio file
                match db.get_audio_file_by_path(&rel_path_str) {
                    Ok(Some(audio_file)) => (vec![audio_file], 0),
                    _ => (Vec::new(), 0),
                }
            } else {
                (files_in_dir, selected_idx)
            }
        }
    }
}

define_domain_query! {
    /// Load organizable inbox directories for the organize workflow.
    ///
    /// Returns the grouped directories (data only). The caller constructs
    /// the full `InboxOrganizeState` with navigator and UI state.
    GetInboxOrganizeData {
        config: crate::config::Config,
    } => Vec<crate::ui::inbox_organize::InboxDirectory>, uncached, |s, db| {
        let files = db.get_organizable_inbox_files().unwrap_or_default();
        if files.is_empty() {
            return Vec::new();
        }
        let inbox_dir = s.config.inbox_dir();
        let granularity = s.config.opinions.inbox_organize.directory_granularity;
        crate::ui::inbox_organize::group_into_directories(&files, &inbox_dir, granularity)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, ReadOnlyDb};

    /// Create a test database with full schema, return it for query testing.
    fn test_db() -> Database {
        Database::open_in_memory()
    }

    // -- Summary queries: empty DB returns defaults --

    #[test]
    fn get_insights_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetInsights.execute(&read_db);
        assert_eq!(result.bucket_corpus.files_in_corpus, 0);
    }

    #[test]
    fn get_inbox_overview_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetInboxOverview.execute(&read_db);
        assert_eq!(result.file_in_inbox, 0);
    }

    #[test]
    fn get_deploy_status_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetDeployStatus.execute(&read_db);
        assert!(!result.needs_action);
        assert!(result.library_file_counts.is_empty());
    }

    #[test]
    fn get_edit_history_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetEditHistory.execute(&read_db);
        assert!(result.sessions.is_empty());
    }

    #[test]
    fn get_external_matches_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetExternalMatches.execute(&read_db);
        assert!(result.confidence_buckets.is_empty());
        assert_eq!(result.packing_perfect_count, 0);
    }

    #[test]
    fn get_packing_dirs_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetPackingDirs.execute(&read_db);
        assert!(result.file_paths.is_empty());
        assert!(result.dir_categories.is_empty());
    }

    // -- Detail queries: empty DB returns empty vecs --

    #[test]
    fn get_oob_sync_files_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetOobSyncFiles.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_oob_files_bucketed_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetOobFilesBucketed.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_moved_files_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetMovedFiles.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_missing_album_single_signals_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetMissingAlbumSingleSignals.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_session_edit_history_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetSessionEditHistory {
            session_id: "nonexistent".to_string(),
        }
        .execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_all_edit_history_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetAllEditHistory.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_compound_signal_groups_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetCompoundSignalGroups {
            safe_only: false,
            tag_filter: None,
        }
        .execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_inbox_compound_signal_groups_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetInboxCompoundSignalGroups.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_packing_knots_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetPackingKnots.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_packing_inode_paths_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetPackingInodePaths.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_inconsistent_album_artist_keys_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetInconsistentAlbumArtistKeys.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_tag_canonicity_keys_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetTagCanonicityKeys { tag_filter: None }.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_inbox_tag_canonicity_keys_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetInboxTagCanonicityKeys.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_disc_extraction_with_paths_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetDiscExtractionData { map_letters_to_numbers: false }.execute(&read_db);
        assert!(result.groups.is_empty());
    }

    // -- Wave 3 modal init loaders: empty DB returns defaults --

    #[test]
    fn get_missing_file_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetMissingFileData.execute(&read_db);
        assert!(result.restorable.is_empty());
        assert!(result.non_restorable.is_empty());
    }

    #[test]
    fn get_missing_directory_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetMissingDirectoryData.execute(&read_db);
        assert!(result.directories.is_empty());
    }

    #[test]
    fn get_corrupt_file_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetCorruptFileData.execute(&read_db);
        assert!(result.files.is_empty());
    }

    #[test]
    fn get_subpar_duplicate_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetSubparDuplicateData.execute(&read_db);
        assert!(result.files.is_empty());
    }

    #[test]
    fn get_directory_cluster_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetDirectoryClusterData.execute(&read_db);
        assert!(result.clusters.is_empty());
    }

    #[test]
    fn get_release_overlap_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetReleaseOverlapData.execute(&read_db);
        assert!(result.clusters.is_empty());
    }

    #[test]
    fn get_shit_format_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetShitFormatData.execute(&read_db);
        assert!(result.lossless_files.is_empty());
        assert!(result.lossy_files.is_empty());
    }

    #[test]
    fn get_inbox_corpus_match_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetInboxCorpusMatchData { bitrate_fuzz_percent: 5.0 }.execute(&read_db);
        assert!(result.entries.is_empty());
    }

    #[test]
    fn get_manual_review_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetManualReviewData {
            kind: manual_review_modal::types::ReviewKind::RedundantDuplicate,
        }.execute(&read_db);
        assert!(result.groups.is_empty());
    }

    #[test]
    fn get_corpus_tags_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetCorpusTags { inode: 999 }.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_packing_browser_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetPackingBrowserData { category_prefix: "full_match".to_string() }.execute(&read_db);
        assert!(result.packed.is_empty());
        assert!(result.packing.is_empty());
    }

    #[test]
    fn get_unsolved_packing_data_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetUnsolvedPackingData { category: "conflict".to_string() }.execute(&read_db);
        assert!(result.is_empty());
    }

    // -- Wave 4 composite queries: empty DB returns defaults --

    #[test]
    fn get_audio_files_by_inodes_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetAudioFilesByInodes {
            inodes: vec![1, 2, 3],
            zone: crate::db::types::Zone::Corpus,
        }.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_missing_tag_audio_files_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetMissingTagAudioFiles.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_all_audio_files_with_tags_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetAllAudioFilesWithTags {
            zone: crate::db::types::Zone::Corpus,
            include_library: false,
        }.execute(&read_db);
        assert!(result.is_empty());
    }

    #[test]
    fn get_session_edit_detail_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetSessionEditDetail { session_id: "nonexistent".to_string() }.execute(&read_db);
        assert!(result.edits.is_empty());
        assert!(result.inode_paths.is_empty());
    }

    #[test]
    fn get_current_tag_values_empty_db() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);
        let result = GetCurrentTagValues {
            queries: vec![(999, "ARTIST".to_string())],
        }.execute(&read_db);
        assert_eq!(result.len(), 1);
        assert!(result[0].is_none());
    }

    // -- Serialize contract: all responses must serialize to JSON --

    #[test]
    fn all_responses_serialize() {
        let db = test_db();
        let read_db = ReadOnlyDb::new(&db);

        // Summary queries
        serde_json::to_string(&GetInsights.execute(&read_db)).unwrap();
        serde_json::to_string(&GetInboxOverview.execute(&read_db)).unwrap();
        serde_json::to_string(&GetDeployStatus.execute(&read_db)).unwrap();
        serde_json::to_string(&GetEditHistory.execute(&read_db)).unwrap();
        serde_json::to_string(&GetExternalMatches.execute(&read_db)).unwrap();
        serde_json::to_string(&GetPackingDirs.execute(&read_db)).unwrap();

        // Detail queries
        serde_json::to_string(&GetOobSyncFiles.execute(&read_db)).unwrap();
        serde_json::to_string(&GetOobFilesBucketed.execute(&read_db)).unwrap();
        serde_json::to_string(&GetMovedFiles.execute(&read_db)).unwrap();
        serde_json::to_string(&GetMissingAlbumSingleSignals.execute(&read_db)).unwrap();
        serde_json::to_string(&GetAllEditHistory.execute(&read_db)).unwrap();
        serde_json::to_string(&GetCompoundSignalGroups {
            safe_only: false,
            tag_filter: None,
        }.execute(&read_db)).unwrap();
        serde_json::to_string(&GetInboxCompoundSignalGroups.execute(&read_db)).unwrap();
        serde_json::to_string(&GetPackingKnots.execute(&read_db)).unwrap();
        serde_json::to_string(&GetPackingInodePaths.execute(&read_db)).unwrap();
        serde_json::to_string(&GetInconsistentAlbumArtistKeys.execute(&read_db)).unwrap();
        serde_json::to_string(&GetTagCanonicityKeys { tag_filter: None }.execute(&read_db)).unwrap();
        serde_json::to_string(&GetInboxTagCanonicityKeys.execute(&read_db)).unwrap();
        serde_json::to_string(&GetDiscExtractionData { map_letters_to_numbers: false }.execute(&read_db)).unwrap();

        // Modal init loaders
        serde_json::to_string(&GetMissingFileData.execute(&read_db)).unwrap();
        serde_json::to_string(&GetMissingDirectoryData.execute(&read_db)).unwrap();
        serde_json::to_string(&GetCorruptFileData.execute(&read_db)).unwrap();
        serde_json::to_string(&GetSubparDuplicateData.execute(&read_db)).unwrap();
        serde_json::to_string(&GetDirectoryClusterData.execute(&read_db)).unwrap();
        serde_json::to_string(&GetReleaseOverlapData.execute(&read_db)).unwrap();
        serde_json::to_string(&GetShitFormatData.execute(&read_db)).unwrap();
        serde_json::to_string(&GetInboxCorpusMatchData { bitrate_fuzz_percent: 5.0 }.execute(&read_db)).unwrap();
        serde_json::to_string(&GetManualReviewData {
            kind: manual_review_modal::types::ReviewKind::RedundantDuplicate,
        }.execute(&read_db)).unwrap();
        serde_json::to_string(&GetCorpusTags { inode: 1 }.execute(&read_db)).unwrap();
        serde_json::to_string(&GetPackingBrowserData { category_prefix: "x".to_string() }.execute(&read_db)).unwrap();
        serde_json::to_string(&GetUnsolvedPackingData { category: "x".to_string() }.execute(&read_db)).unwrap();

        // Composite queries
        serde_json::to_string(&GetMissingTagAudioFiles.execute(&read_db)).unwrap();
        serde_json::to_string(&GetSessionEditDetail { session_id: "x".to_string() }.execute(&read_db)).unwrap();
        serde_json::to_string(&GetCurrentTagValues { queries: vec![] }.execute(&read_db)).unwrap();
    }
}
