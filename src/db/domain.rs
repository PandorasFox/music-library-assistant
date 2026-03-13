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
//! ## Layout
//!
//! Query struct definitions, `DomainQueryPayload`, `DomainQueryResult`, and
//! `ProtocolQuery` impls all live in **mm-meta** and are re-exported here.
//! This file provides only:
//! - The `DomainQuery` trait (requires `ReadOnlyDb`, which lives in mm)
//! - `impl DomainQuery for ...` for every query (via `impl_domain_query!`)
//! - `dispatch_domain_query()` (server-side exhaustive dispatch)
//! - Helper functions used by execute bodies

// Re-export query structs and protocol bridge types from mm-meta.
pub use mm_meta::domain_queries::*;

// Re-export domain query response types from mm-meta.
pub use mm_meta::domain_query_types::*;

use serde::Serialize;

use crate::db::ReadOnlyDb;

// ============================================================================
// Core Trait
// ============================================================================

/// Every domain query implements this. The trait is the contract
/// that `impl_domain_query!` generates against.
///
/// Implementors must follow the uniformity rules:
/// 1. All inputs are fields on `self`
/// 2. All outputs are in `Response`
/// 3. No side channels, no `&mut`, no extra context parameters
/// 4. Errors handled internally — return defaults, never `Result`
pub trait DomainQuery: Send + 'static {
    /// The response type. Must be serializable for wire transport.
    type Response: std::fmt::Debug + Clone + Serialize + serde::de::DeserializeOwned + Send + 'static;

    /// Execute the query against a read-only database connection.
    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response;
}

// ============================================================================
// Macro: impl_domain_query!
// ============================================================================

/// Generates a `DomainQuery` impl for an existing query struct (defined in mm-meta).
///
/// Forms:
/// ```ignore
/// // Simple: unit struct, single db method call, unwrap_or_default
/// impl_domain_query! { GetFoo => FooData, db.get_foo_data() }
///
/// // Body: unit struct, custom execute logic with `db` in scope
/// impl_domain_query! { GetBar => BarData, |db| { ... } }
///
/// // Parameterized body: struct with fields, `s` = &self, `db` = &ReadOnlyDb
/// impl_domain_query! { GetBaz => BazData, |s, db| { ... } }
/// ```
macro_rules! impl_domain_query {
    // Simple form: single db method, unwrap_or_default
    (
        $name:ident => $response:ty, db.$method:ident()
    ) => {
        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
                db.$method().unwrap_or_default()
            }
        }
    };

    // Body form: custom execute expression with db closure
    (
        $name:ident => $response:ty, |$db:ident| $body:block
    ) => {
        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, $db: &ReadOnlyDb<'_>) -> Self::Response {
                $body
            }
        }
    };

    // Parameterized body form: struct with fields + custom execute body
    (
        $name:ident => $response:ty, |$s:ident, $db:ident| $body:block
    ) => {
        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, $db: &ReadOnlyDb<'_>) -> Self::Response {
                let $s = &self;
                $body
            }
        }
    };
}

// ============================================================================
// Imports for execute bodies
// ============================================================================

use crate::meta::signals::data::CompoundGroup;
use crate::meta::views::{
    BucketedOobFile, DeployStatus, EditHistoryData, EditHistoryExportRow, ExternalMatchesData,
    InboxOverviewData, InsightsData, MovedFileInfo, OobSyncFile,
};

use crate::db::modal_loaders;
use mm_meta::views::canonicity_compound::CanonicitySignalKind;
use mm_meta::views::cluster_deploy::{
    DeployModalData, DirectoryClusterModalData, ShitFormatModalData,
};
use mm_meta::views::health_modals::{
    CorruptFileModalData, MissingDirectoryModalData, MissingFileModalData,
    SubparDuplicateModalData,
};
use mm_meta::views::review_match::{
    InboxCorpusMatchModalData, ManualReviewData, RecordingDetail, RecordingSummary,
};
use mm_meta::views::startup_organize::{IntakeConfirmationState, IntakeSource, InboxDirectory};
use mm_meta::views::canonicity_compound::{CompoundSplitDataV2, TagCanonicalityModalDataV2};

// ============================================================================
// Summary Queries
// ============================================================================

impl_domain_query! {
    GetInsights => InsightsData, db.get_insights_data()
}

impl_domain_query! {
    GetInboxOverview => InboxOverviewData, db.get_inbox_overview_data()
}

impl_domain_query! {
    GetDeployStatus => DeployStatus, db.get_deploy_status()
}

impl_domain_query! {
    GetEditHistory => EditHistoryData, |db| {
        let sessions = db.get_edit_sessions().unwrap_or_default();
        EditHistoryData { sessions }
    }
}

impl_domain_query! {
    GetExternalMatches => ExternalMatchesData, db.get_external_matches_data()
}

impl_domain_query! {
    GetPackingDirs => PackingDirsData, |db| {
        let file_paths = db.get_packing_assigned_paths().unwrap_or_default();
        let dir_categories = db.get_packing_directory_categories().unwrap_or_default();
        PackingDirsData { file_paths, dir_categories }
    }
}

// ============================================================================
// Detail Queries (Wave 1: simple return types, no inode resolution)
// ============================================================================

impl_domain_query! {
    GetOobSyncFiles => Vec<OobSyncFile>, db.get_oob_sync_files()
}

impl_domain_query! {
    GetOobFilesBucketed => Vec<BucketedOobFile>, db.get_oob_files_bucketed()
}

impl_domain_query! {
    GetMovedFiles => Vec<MovedFileInfo>, db.get_moved_files()
}

impl_domain_query! {
    GetMissingAlbumSingleSignals => Vec<MissingAlbumSingleSignalWire>, |db| {
        db.get_missing_album_single_signals()
            .unwrap_or_default()
            .into_iter()
            .map(|s| MissingAlbumSingleSignalWire {
                key: s.key,
                data: s.data,
            })
            .collect()
    }
}

impl_domain_query! {
    GetSessionEditHistory => Vec<EditHistoryExportRow>, |s, db| {
        db.get_session_edit_history(&s.session_id)
            .unwrap_or_default()
    }
}

impl_domain_query! {
    GetAllEditHistory => Vec<EditHistoryExportRow>, db.get_all_edit_history()
}

impl_domain_query! {
    GetCompoundSignalGroups => Vec<CompoundGroup>, |s, db| {
        db.get_compound_signal_groups_by_safety(s.safe_only, s.tag_filter.as_deref())
            .unwrap_or_default()
    }
}

impl_domain_query! {
    GetInboxCompoundSignalGroups => Vec<CompoundGroup>, db.get_inbox_compound_signal_groups()
}

impl_domain_query! {
    GetPackingKnots => Vec<crate::meta::signals::data::PackingKnotData>, db.get_packing_knots()
}

impl_domain_query! {
    GetPackingInodePaths => Vec<(i64, String)>, db.get_packing_inode_paths()
}

// ============================================================================
// Detail Queries (Wave 2: signal key queries for canonicity resolution)
// ============================================================================

impl_domain_query! {
    GetInconsistentAlbumArtistKeys => Vec<String>, |db| {
        use crate::meta::signals::data::InconsistentAlbumArtistSignal;
        db.aggregate_signal_keys::<InconsistentAlbumArtistSignal>().unwrap_or_default()
    }
}

impl_domain_query! {
    GetTagCanonicityKeys => Vec<String>, |s, db| {
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

impl_domain_query! {
    GetInboxTagCanonicityKeys => Vec<String>, |db| {
        use crate::meta::signals::data::InboxTagCanonicitySignal;
        db.aggregate_signal_keys::<InboxTagCanonicitySignal>().unwrap_or_default()
    }
}

impl_domain_query! {
    GetDiscExtractionData => DiscExtractionModalData, |s, db| {
        use crate::meta::signals::data::DiscExtractionSource;

        let signals = db.get_disc_extraction_signals().unwrap_or_default();
        if signals.is_empty() {
            return DiscExtractionModalData { groups: Vec::new() };
        }

        let all_inodes: Vec<i64> = signals
            .iter()
            .flat_map(|sig| sig.data.inodes.iter().copied())
            .collect();
        let path_map = db
            .get_file_paths_batch(crate::db::types::Zone::Corpus, &all_inodes)
            .unwrap_or_default();
        let path_lookup = |inode: i64| {
            path_map.get(&inode).cloned().unwrap_or_else(|| format!("<inode {}>", inode))
        };

        let groups = signals
            .into_iter()
            .map(|sig| match &sig.data.source {
                DiscExtractionSource::Album {
                    original_album,
                    cleaned_album,
                    disc_number,
                } => {
                    let files: Vec<DiscFileEntry> = sig
                        .data
                        .inodes
                        .iter()
                        .map(|&inode| DiscFileEntry {
                            inode,
                            path: path_lookup(inode),
                            original_value: original_album.clone(),
                            cleaned_value: cleaned_album.clone(),
                            source_tag: "ALBUM".to_string(),
                        })
                        .collect();
                    DiscExtractionGroup {
                        description: format!("Album \"{}\" → Disc {}", original_album, disc_number),
                        disc_value: disc_number.clone(),
                        files,
                    }
                }
                DiscExtractionSource::TrackNumber {
                    disc_prefix,
                    album,
                    album_artist,
                    per_file,
                } => {
                    let disc_value = if s.map_letters_to_numbers {
                        mm_meta::domain_queries::letter_to_number(disc_prefix)
                    } else {
                        disc_prefix.clone()
                    };
                    let files: Vec<DiscFileEntry> = per_file
                        .iter()
                        .map(|tf| DiscFileEntry {
                            inode: tf.inode,
                            path: path_lookup(tf.inode),
                            original_value: tf.original_value.clone(),
                            cleaned_value: tf.cleaned_digits.clone(),
                            source_tag: "TRACKNUMBER".to_string(),
                        })
                        .collect();
                    let context = if album_artist.is_empty() {
                        album.clone()
                    } else {
                        format!("{} — {}", album_artist, album)
                    };
                    DiscExtractionGroup {
                        description: format!(
                            "TrackNumber prefix \"{}\" in {}",
                            disc_prefix, context
                        ),
                        disc_value,
                        files,
                    }
                }
            })
            .collect();

        DiscExtractionModalData { groups }
    }
}

// ============================================================================
// Detail Queries (Wave 3: modal init loaders)
// ============================================================================

impl_domain_query! {
    GetMissingFileData => MissingFileModalData, |db| {
        modal_loaders::load_missing_file_data(db).ok().unwrap_or_default()
    }
}

impl_domain_query! {
    GetMissingDirectoryData => MissingDirectoryModalData, |db| {
        modal_loaders::load_missing_directory_data(db).ok().unwrap_or_default()
    }
}

impl_domain_query! {
    GetCorruptFileData => CorruptFileModalData, |db| {
        modal_loaders::load_corrupt_file_data(db).ok().unwrap_or_default()
    }
}

impl_domain_query! {
    GetSubparDuplicateData => SubparDuplicateModalData, |db| {
        modal_loaders::load_subpar_duplicate_data(db).ok().unwrap_or_default()
    }
}

impl_domain_query! {
    GetDirectoryClusterData => DirectoryClusterModalData, |db| {
        modal_loaders::load_directory_cluster_data(db).ok().unwrap_or_default()
    }
}

impl_domain_query! {
    GetReleaseOverlapData => DirectoryClusterModalData, |db| {
        modal_loaders::load_release_overlap_data(db).ok().unwrap_or_default()
    }
}

impl_domain_query! {
    GetShitFormatData => ShitFormatModalData, |db| {
        modal_loaders::load_shit_format_data(db).ok().unwrap_or_default()
    }
}

impl_domain_query! {
    GetInboxCorpusMatchData => InboxCorpusMatchModalData, |s, db| {
        modal_loaders::load_inbox_corpus_match_data(db, s.bitrate_fuzz_percent)
            .ok()
            .unwrap_or_default()
    }
}

impl_domain_query! {
    GetDeployData => DeployModalData, |s, db| {
        modal_loaders::load_deploy_data(db, s.config.as_ref())
            .unwrap_or_default()
    }
}

impl_domain_query! {
    GetManualReviewData => ManualReviewData, |s, db| {
        modal_loaders::load_manual_review_data(db, s.kind)
            .ok()
            .unwrap_or_default()
    }
}

impl_domain_query! {
    GetCorpusTags => Vec<(String, String)>, |s, db| {
        db.get_tags::<crate::zones::CorpusZone>(s.inode)
            .unwrap_or_default()
            .into_iter()
            .map(|t| (t.tag_name, t.tag_value))
            .collect()
    }
}

impl_domain_query! {
    GetPackingBrowserData => PackingBrowserData, |s, db| {
        PackingBrowserData {
            packed: db.get_packed_releases_by_category(&s.category_prefix).unwrap_or_default(),
            packing: db.get_release_packing_signal_data().unwrap_or_default(),
            unfilled: db.get_unfilled_release_slot_signal_data().unwrap_or_default(),
            alternatives: db.get_alternative_release_packing_data().unwrap_or_default(),
            va_overrides: db.get_various_artists_override_data().unwrap_or_default(),
        }
    }
}

impl_domain_query! {
    GetUnsolvedPackingData => Vec<(i64, String, crate::meta::signals::data::UnmatchedCorpusTrackData)>, |s, db| {
        db.get_unmatched_corpus_track_signal_data_by_category(&s.category)
            .unwrap_or_default()
    }
}

// ============================================================================
// Detail Queries (Wave 4: composite queries collapsed into single execute)
// ============================================================================

impl_domain_query! {
    GetAudioFilesByInodes => Vec<crate::db::types::AudioFile>, |s, db| {
        db.get_audio_files_by_inodes(&s.inodes, s.zone)
            .unwrap_or_default()
    }
}

impl_domain_query! {
    GetMissingTagAudioFiles => Vec<crate::db::types::AudioFile>, |db| {
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

impl_domain_query! {
    GetAllAudioFilesWithTags => Vec<AudioFileWithTags>, |s, db| {
        db.get_all_audio_files_with_tags(s.zone, s.include_library)
            .unwrap_or_default()
    }
}

impl_domain_query! {
    GetSessionEditDetail => SessionEditDetail, |s, db| {
        let edits = db.get_session_edits(&s.session_id).unwrap_or_default();
        let inodes: Vec<i64> = edits.iter().map(|e| e.inode).collect();
        let inode_paths = db
            .get_file_paths_batch(crate::db::types::Zone::Corpus, &inodes)
            .unwrap_or_default();
        SessionEditDetail { edits, inode_paths }
    }
}

impl_domain_query! {
    GetCurrentTagValues => Vec<Option<String>>, |s, db| {
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

impl_domain_query! {
    GetIntakeConfirmation => Option<IntakeConfirmationState>, |s, db| {
        match s.zone {
            Some(crate::db::types::Zone::Corpus) => {
                modal_loaders::gather_intake_zone::<crate::zones::CorpusZone>(db, s.source)
            }
            Some(crate::db::types::Zone::Inbox) => {
                modal_loaders::gather_intake_zone::<crate::zones::InboxZone>(db, s.source)
            }
            Some(_) => None,
            None => modal_loaders::gather_intake_startup(db),
        }
    }
}

impl_domain_query! {
    GetCompoundSplitGroupData => Option<CompoundSplitDataV2>, |s, db| {
        modal_loaders::load_compound_split_data(&s.group, db, s.zone)
    }
}

impl_domain_query! {
    GetTagCanonicitySignalData => Option<TagCanonicalityModalDataV2>, |s, db| {
        load_tag_canonicity_signal_data(&s.signal_key, s.kind, db)
    }
}

/// Load typed tag canonicity signal data by key and kind.
///
/// Extracted from `App::load_typed_signal_data` so it can be called
/// from the domain query without needing `&self`.
fn load_tag_canonicity_signal_data(
    key: &str,
    kind: CanonicitySignalKind,
    read_db: &ReadOnlyDb,
) -> Option<TagCanonicalityModalDataV2> {
    match kind {
        CanonicitySignalKind::TagCanonicity => {
            let signal = read_db.get_tag_canonicity_signal(key).ok()??;
            modal_loaders::load_tag_canonicity_data(&signal, read_db)
        }
        CanonicitySignalKind::InconsistentAlbumArtist => {
            let signal = read_db.get_inconsistent_album_artist_signal(key).ok()??;
            modal_loaders::load_inconsistent_album_artist_data(&signal, read_db)
        }
        CanonicitySignalKind::InboxTagCanonicity => {
            let signal = read_db.get_inbox_tag_canonicity_signal(key).ok()??;
            modal_loaders::load_inbox_tag_canonicity_data(&signal, read_db)
        }
    }
}

impl_domain_query! {
    GetRecordingBatchData => RecordingBatchResult, |s, db| {
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
    // RecordingDetail and RecordingSummary imported at module level from mm_meta::views::review_match
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

impl_domain_query! {
    GetReleaseStagingData => ReleaseStagingData, |s, db| {
        use crate::external::musicbrainz::load_mb_cache_bundle;
        let bundle = load_mb_cache_bundle(db, &s.release_ids, &s.recording_ids);
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

impl_domain_query! {
    GetTagEditorFiles => (Vec<crate::db::types::AudioFile>, usize), |s, db| {
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

impl_domain_query! {
    GetInboxOrganizeData => Vec<InboxDirectory>, |s, db| {
        let files = db.get_organizable_inbox_files().unwrap_or_default();
        if files.is_empty() {
            return Vec::new();
        }
        let inbox_dir = s.config.inbox_dir();
        let granularity = s.config.opinions.inbox_organize.directory_granularity;
        let resolver = mm_meta::paths::PathResolver::from_config(&s.config);
        mm_meta::views::startup_organize::group_into_directories(&files, &inbox_dir, granularity, &resolver)
    }
}

impl_domain_query! {
    GetFileTagValues => Vec<(i64, Vec<(String, String)>)>, |s, db| {
        load_file_tag_values(&s.inodes, s.zone, db)
    }
}

/// Read tags from disk for a batch of audio files.
fn load_file_tag_values(
    inodes: &[i64],
    zone: crate::db::types::Zone,
    db: &ReadOnlyDb,
) -> Vec<(i64, Vec<(String, String)>)> {
    use crate::corpus::paths;
    use crate::corpus::tags::{self as tags};

    let resolver = paths::get_resolver();
    let path_map = db
        .get_file_paths_batch(zone, inodes)
        .unwrap_or_default();

    let mut results = Vec::with_capacity(inodes.len());
    for &inode in inodes {
        let Some(rel_path) = path_map.get(&inode) else {
            results.push((inode, Vec::new()));
            continue;
        };
        let abs_path = resolver.resolve(std::path::Path::new(rel_path));
        let tags = match tags::from_file(&abs_path) {
            Ok(ts) => ts.into_vec(),
            Err(e) => {
                crate::logging::log_error(format!(
                    "Could not read tags from {}: {}", abs_path.display(), e
                ));
                Vec::new()
            }
        };
        results.push((inode, tags));
    }
    results
}

// ============================================================================
// Protocol Bridge — dispatch_domain_query
// ============================================================================

macro_rules! dispatch_domain_query_impl {
    ( $( $query:ident ),+ $(,)? ) => {
        /// Server-side dispatch: execute a domain query payload against a read-only DB.
        /// Exhaustive match ensures compile-time coupling with mm-meta's enum variants.
        pub fn dispatch_domain_query(
            payload: DomainQueryPayload,
            db: &ReadOnlyDb<'_>,
        ) -> DomainQueryResult {
            match payload {
                $( DomainQueryPayload::$query(q) => DomainQueryResult::$query(q.execute(db)), )+
            }
        }
    };
}

dispatch_domain_query_impl! {
    GetInsights,
    GetInboxOverview,
    GetDeployStatus,
    GetEditHistory,
    GetExternalMatches,
    GetPackingDirs,
    GetOobSyncFiles,
    GetOobFilesBucketed,
    GetMovedFiles,
    GetMissingAlbumSingleSignals,
    GetSessionEditHistory,
    GetAllEditHistory,
    GetCompoundSignalGroups,
    GetInboxCompoundSignalGroups,
    GetPackingKnots,
    GetPackingInodePaths,
    GetInconsistentAlbumArtistKeys,
    GetTagCanonicityKeys,
    GetInboxTagCanonicityKeys,
    GetDiscExtractionData,
    GetMissingFileData,
    GetMissingDirectoryData,
    GetCorruptFileData,
    GetSubparDuplicateData,
    GetDirectoryClusterData,
    GetReleaseOverlapData,
    GetShitFormatData,
    GetInboxCorpusMatchData,
    GetDeployData,
    GetManualReviewData,
    GetCorpusTags,
    GetPackingBrowserData,
    GetUnsolvedPackingData,
    GetAudioFilesByInodes,
    GetMissingTagAudioFiles,
    GetAllAudioFilesWithTags,
    GetSessionEditDetail,
    GetCurrentTagValues,
    GetIntakeConfirmation,
    GetCompoundSplitGroupData,
    GetTagCanonicitySignalData,
    GetRecordingBatchData,
    GetReleaseStagingData,
    GetTagEditorFiles,
    GetInboxOrganizeData,
    GetFileTagValues,
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
            kind: mm_meta::views::review_match::ReviewKind::RedundantDuplicate,
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
            kind: mm_meta::views::review_match::ReviewKind::RedundantDuplicate,
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
