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
//! and optional `CachedQuery` impl from a compact declaration. Two forms:
//!
//! ```ignore
//! // Simple: single db method call, unwrap_or_default
//! define_domain_query! {
//!     /// Doc comment
//!     GetFoo => FooData, cached(15), db.get_foo_data()
//! }
//!
//! // Body: custom execute logic with `db` in scope
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

/// Full edit history for a specific session (for export).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct GetSessionEditHistory {
    pub session_id: String,
}

impl DomainQuery for GetSessionEditHistory {
    type Response = Vec<EditHistoryExportRow>;

    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
        db.get_session_edit_history(&self.session_id)
            .unwrap_or_default()
    }
}

/// Full edit history across all sessions (for export).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct GetAllEditHistory;

impl DomainQuery for GetAllEditHistory {
    type Response = Vec<EditHistoryExportRow>;

    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
        db.get_all_edit_history().unwrap_or_default()
    }
}

/// Compound tag signal groups (for compound split resolution).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct GetCompoundSignalGroups {
    pub safe_only: bool,
    pub tag_filter: Option<String>,
}

impl DomainQuery for GetCompoundSignalGroups {
    type Response = Vec<CompoundGroup>;

    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
        db.get_compound_signal_groups_by_safety(self.safe_only, self.tag_filter.as_deref())
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

/// Aggregate signal keys for InconsistentAlbumArtist signals.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct GetInconsistentAlbumArtistKeys;

impl DomainQuery for GetInconsistentAlbumArtistKeys {
    type Response = Vec<String>;

    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
        use crate::meta::signals::data::InconsistentAlbumArtistSignal;
        db.aggregate_signal_keys::<InconsistentAlbumArtistSignal>()
            .unwrap_or_default()
    }
}

/// Aggregate signal keys for TagCanonicity signals, optionally filtered by tag prefix.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct GetTagCanonicityKeys {
    pub tag_filter: Option<String>,
}

impl DomainQuery for GetTagCanonicityKeys {
    type Response = Vec<String>;

    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
        use crate::meta::signals::data::TagCanonicitySignal;
        let all_keys = db
            .aggregate_signal_keys::<TagCanonicitySignal>()
            .unwrap_or_default();
        match self.tag_filter {
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

/// Aggregate signal keys for InboxTagCanonicity signals.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct GetInboxTagCanonicityKeys;

impl DomainQuery for GetInboxTagCanonicityKeys {
    type Response = Vec<String>;

    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
        use crate::meta::signals::data::InboxTagCanonicitySignal;
        db.aggregate_signal_keys::<InboxTagCanonicitySignal>()
            .unwrap_or_default()
    }
}

/// Disc extraction signals with resolved file paths (self-contained detail query).
///
/// Collapses the two-step signal-load + path-resolution pattern into a single query.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct GetDiscExtractionWithPaths;

/// A disc extraction group with resolved file paths instead of raw inodes.
#[derive(serde::Serialize)]
pub struct DiscExtractionGroupResolved {
    pub key: String,
    pub source: crate::meta::signals::data::DiscExtractionSource,
    pub files: Vec<(i64, String)>,
}

impl DomainQuery for GetDiscExtractionWithPaths {
    type Response = Vec<DiscExtractionGroupResolved>;

    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
        let signals = db.get_disc_extraction_signals().unwrap_or_default();
        if signals.is_empty() {
            return Vec::new();
        }

        // Collect all inodes for batch path resolution
        let all_inodes: Vec<i64> = signals
            .iter()
            .flat_map(|s| s.data.inodes.iter().copied())
            .collect();
        let path_map = db
            .get_file_paths_batch(crate::db::types::Zone::Corpus, &all_inodes)
            .unwrap_or_default();

        signals
            .into_iter()
            .map(|s| {
                let files = s
                    .data
                    .inodes
                    .iter()
                    .map(|&inode| {
                        let path = path_map
                            .get(&inode)
                            .cloned()
                            .unwrap_or_else(|| format!("<inode {}>", inode));
                        (inode, path)
                    })
                    .collect();
                DiscExtractionGroupResolved {
                    key: s.key,
                    source: s.data.source,
                    files,
                }
            })
            .collect()
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
        let result = GetDiscExtractionWithPaths.execute(&read_db);
        assert!(result.is_empty());
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
        serde_json::to_string(&GetDiscExtractionWithPaths.execute(&read_db)).unwrap();
    }
}
