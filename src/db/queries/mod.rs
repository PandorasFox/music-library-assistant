//! Database operations and queries.
//!
//! All SQLite operations are organized into domain-specific submodules:
//! - `files`: Inode-based queries for files/audio_info/corpus_tags tables
//! - `health`: Health issues, summaries
//! - `metadata`: App metadata, tag canonicalization
//! - `library_scan`: Library scanning state

pub mod auth;
pub mod external;
pub mod files;
mod health;
mod library_scan;
mod metadata;

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use crate::config;

// Types are re-exported from db/mod.rs, not here.
// This module only exposes the Database struct.

// ============================================================================
// Path Pattern Helpers
// ============================================================================

/// Escape SQL LIKE wildcard characters in a string.
///
/// In SQLite LIKE patterns, `%` matches any sequence and `_` matches any single
/// character. When using file paths in LIKE patterns, these must be escaped to
/// match literally.
///
/// Uses `\` as the escape character. **Important**: Queries using these patterns
/// MUST include `ESCAPE '\'` in the LIKE clause.
fn escape_like_wildcards(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '%' | '_' | '\\' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}

/// Build a LIKE pattern for matching files within a directory.
///
/// Normalizes the directory path (strips trailing slashes), escapes LIKE wildcards,
/// and returns a pattern that matches files directly within or nested under the directory.
///
/// Pattern format: `{escaped_dir}/%` - requires a path separator after the directory,
/// preventing matches on sibling directories with shared prefixes.
///
/// **Important**: Queries using this pattern MUST include `ESCAPE '\'`.
///
/// # Example
/// ```ignore
/// let pattern = dir_like_pattern("/path/to/Album_Name");
/// // Returns: "/path/to/Album\\_Name/%"
/// // Use: WHERE path LIKE ?1 ESCAPE '\'
/// // Matches: "/path/to/Album_Name/song.mp3"
/// // Does NOT match: "/path/to/Album/Name/song.mp3" (underscore matched as wildcard)
/// ```
pub(crate) fn dir_like_pattern(dir: &std::path::Path) -> String {
    let dir_str = dir.to_string_lossy();
    let normalized = dir_str.trim_end_matches('/');
    let escaped = escape_like_wildcards(normalized);
    format!("{}/%", escaped)
}

/// Build a LIKE pattern from a string directory path.
///
/// Same as `dir_like_pattern` but accepts a string slice directly.
/// **Important**: Queries using this pattern MUST include `ESCAPE '\'`.
pub(crate) fn dir_like_pattern_str(dir: &str) -> String {
    let normalized = dir.trim_end_matches('/');
    let escaped = escape_like_wildcards(normalized);
    format!("{}/%", escaped)
}

/// Central database connection wrapper.
///
/// ## Connection Access
///
/// The `conn` field is private. Normal code should use:
/// - Query methods on this struct for reads
/// - `write_thread::signal_sender()` for writes
///
/// Raw connection access via `conn()` is only for:
/// - `db_thread.rs` - the single write connection
/// - `migration.rs` - schema migrations (pre-Witch infrastructure)
pub struct Database {
    conn: Connection,
}

#[cfg(test)]
impl Database {
    /// Create an in-memory database with full schema for testing.
    ///
    /// The connection is read-write (no `query_only` pragma) so tests can
    /// insert fixture data, then wrap in `ReadOnlyDb` for query testing.
    pub fn open_in_memory() -> Self {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;",
        )
        .unwrap();
        let db = Database { conn };
        db.initialize_schema()
            .expect("failed to initialize test schema");
        db
    }
}

impl Database {
    /// Raw connection access - **INTERNAL USE ONLY**.
    ///
    /// This method exists for:
    /// - `db_thread.rs` (the authorized write thread)
    /// - `migration.rs` (infrastructure migrations)
    ///
    /// **DO NOT USE** from UI code, mutation executors, or computations.
    /// Those should use query methods or `signal_sender()`.
    #[doc(hidden)]
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}

impl Database {
    /// Open a read-write database connection.
    ///
    /// **SEALED: Only callable from authorized locations:**
    /// - `write_thread::spawn()` - the one true write connection
    /// - `startup/first_time_setup.rs` - initial database creation
    /// - `startup/migrations.rs` - pre-Witch schema migrations
    ///
    /// If you're trying to call this elsewhere, you're violating architecture.
    /// - For writes: Use `write_thread::signal_sender()`
    /// - For reads: Use `witch.read_db()` (returns `ReadOnlyDb`)
    pub(in crate::db) fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open database")?;

        // Performance tuning for flash storage
        // Using execute_batch to handle PRAGMAs uniformly (some return rows, some don't)
        let cache_kb = config::get_db_cache_kb();
        conn.execute_batch(&format!(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA cache_size = {};
             PRAGMA temp_store = MEMORY;
             PRAGMA foreign_keys = ON;",
            cache_kb
        ))
        .context("Failed to set database pragmas")?;

        let db = Database { conn };

        // Only initialize schema on truly new databases (no tables yet).
        // Existing databases get schema changes through the migration system.
        let table_count: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'files'",
            [],
            |row| row.get(0),
        )?;
        if table_count == 0 {
            db.initialize_schema()?;
        }

        Ok(db)
    }

    /// Open a read-only database connection.
    ///
    /// Uses SQLite's `PRAGMA query_only = ON` to prevent any writes.
    /// This is the correct way for UI code to access the database.
    ///
    /// Note: Does not initialize schema (read-only connections cannot create tables).
    /// The database must already exist and have the correct schema.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open database")?;

        // Using execute_batch to handle PRAGMAs uniformly
        let cache_kb = config::get_db_cache_kb();
        conn.execute_batch(&format!(
            "PRAGMA busy_timeout = 5000;
             PRAGMA cache_size = {};
             PRAGMA foreign_keys = ON;
             PRAGMA query_only = ON;",
            cache_kb
        ))
        .context("Failed to set database pragmas")?;

        Ok(Database { conn })
    }

    // Schema initialization and version management: see db/schema.rs

    // ========================================================================
    // Generic Bincode BLOB Readers
    // ========================================================================

    /// Query a signal table's `data` BLOB column and deserialize each row.
    ///
    /// Silently skips rows that fail to deserialize (stale schema).
    pub(crate) fn query_signal_blobs<T: serde::de::DeserializeOwned>(
        &self,
        sql: &str,
    ) -> Result<Vec<T>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        let mut results = Vec::new();
        for blob in rows.flatten() {
            if let Ok(data) = bincode::deserialize(&blob) {
                results.push(data);
            }
        }
        Ok(results)
    }

    /// Query a signal table for `(key TEXT, data BLOB)` and deserialize each row.
    ///
    /// Silently skips rows that fail to deserialize.
    pub(crate) fn query_signal_key_blobs<T: serde::de::DeserializeOwned>(
        &self,
        sql: &str,
    ) -> Result<Vec<(String, T)>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows.flatten() {
            if let Ok(data) = bincode::deserialize(&row.1) {
                results.push((row.0, data));
            }
        }
        Ok(results)
    }

    /// Query a signal table for `(inode, path, data BLOB)` and deserialize each row.
    ///
    /// Silently skips rows that fail to deserialize.
    pub(crate) fn query_signal_inode_blobs<T: serde::de::DeserializeOwned>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::types::ToSql],
    ) -> Result<Vec<(i64, String, T)>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?))
        })?;
        let mut results = Vec::new();
        for row in rows.flatten() {
            if let Ok(data) = bincode::deserialize(&row.2) {
                results.push((row.0, row.1, data));
            }
        }
        Ok(results)
    }
}

// ============================================================================
// Read-Only Database Wrapper
// ============================================================================

/// A read-only view of the database.
///
/// This wrapper exposes commonly-used read methods directly, providing
/// compile-time safety that UI code cannot accidentally attempt write
/// operations. The underlying connection also has `PRAGMA query_only = ON`
/// for runtime protection.
///
/// # Usage
///
/// UI code receives `&ReadOnlyDb` from `Witch::read_db()` and should use
/// the exposed methods directly:
///
/// ```ignore
/// let read_db = witch.read_db();
/// let audio_files = read_db.get_all_audio_files(Zone::Corpus)?;
/// let signals = read_db.get_signals(None)?;
/// ```
///
/// For specialized internal queries not exposed here, use `inner()` (crate-only).
///
/// # Naming Convention
///
/// Variables holding this type should be named `read_db` to make their
/// read-only nature clear in code.
pub struct ReadOnlyDb<'a> {
    db: &'a Database,
}

// Generates simple forwarding methods on ReadOnlyDb that delegate to self.db.
// Methods that use self.db.conn() directly are hand-written below the macro invocation.
macro_rules! delegate_read {
    ($(
        $(#[$attr:meta])*
        fn $name:ident($($arg:ident : $ty:ty),* $(,)?) -> $ret:ty;
    )*) => {
        $(
            $(#[$attr])*
            pub fn $name(&self, $($arg: $ty),*) -> $ret {
                self.db.$name($($arg),*)
            }
        )*
    };
}

impl<'a> ReadOnlyDb<'a> {
    /// Create a read-only view of a database.
    ///
    /// This should only be called from `Witch::read_db()` which ensures
    /// the underlying connection has `PRAGMA query_only = ON`.
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    // =========================================================================
    // AudioFile Queries
    // =========================================================================

    delegate_read! {
        fn get_all_audio_files(source: super::types::Zone, with_fingerprints: bool) -> Result<Vec<super::types::AudioFile>>;
        fn get_audio_file_by_path(path: &str) -> Result<Option<super::types::AudioFile>>;
        fn get_audio_file_by_inode(inode: i64, source: super::types::Zone) -> Result<Option<super::types::AudioFile>>;
        fn get_audio_files_by_inodes(inodes: &[i64], source: super::types::Zone) -> Result<Vec<super::types::AudioFile>>;
        fn get_audio_files_by_path_prefix(path_prefix: &str) -> Result<Vec<super::types::AudioFile>>;
        fn get_audio_files_for_tag_editing(dir_path: &std::path::Path) -> Result<Vec<super::types::AudioFile>>;
        fn get_all_audio_files_with_tags(source: super::types::Zone, with_fingerprints: bool) -> Result<Vec<files::AudioFileWithTags>>;
        fn get_tags_for_zone(inode: i64, zone: super::types::Zone) -> Result<Vec<super::types::AudioTag>>;
        fn get_audio_info(inode: i64) -> Result<Option<super::types::AudioInfo>>;
        fn get_has_pictures(inode: i64) -> Result<bool>;
    }

    // =========================================================================
    // Signal Queries
    // =========================================================================

    delegate_read! {
        fn get_healthy_file_signals() -> Result<Vec<crate::meta::signals::data::HealthyFileSignal>>;
        fn get_tag_canonicity_signal(key: &str) -> Result<Option<crate::meta::signals::data::TagCanonicitySignal>>;
        fn get_inconsistent_album_artist_signal(key: &str) -> Result<Option<crate::meta::signals::data::InconsistentAlbumArtistSignal>>;
        fn get_compound_tag_signal(inode: i64) -> Result<Option<crate::meta::signals::data::CompoundTagSignal>>;
        fn get_inbox_compound_tag_signal(inode: i64) -> Result<Option<crate::meta::signals::data::InboxCompoundTagSignal>>;
        fn get_cross_source_overlap_signals() -> Result<Vec<crate::meta::signals::data::CrossSourceOverlapSignal>>;
        fn get_release_overlap_signals() -> Result<Vec<crate::meta::signals::data::ReleaseOverlapSignal>>;
        fn get_fingerprint_overlap_signals() -> Result<Vec<crate::meta::signals::data::FingerprintOverlapSignal>>;
        fn get_missing_tag_signals() -> Result<Vec<crate::meta::signals::data::MissingTagSignal>>;
        fn get_missing_album_single_signals() -> Result<Vec<crate::meta::signals::data::MissingAlbumSingleSignal>>;
        fn get_disc_extraction_signals() -> Result<Vec<crate::meta::signals::data::DiscExtractionSignal>>;
        fn get_tracknumber_values_with_context() -> Result<Vec<(i64, String, String, String)>>;
        fn get_inbox_tag_canonicity_signal(key: &str) -> Result<Option<crate::meta::signals::data::InboxTagCanonicitySignal>>;
        fn get_compound_signal_groups_by_safety(safe_only: bool, tag_filter: Option<&str>) -> Result<Vec<crate::meta::signals::data::CompoundGroup>>;
        fn get_inbox_compound_signal_groups() -> Result<Vec<crate::meta::signals::data::CompoundGroup>>;
        fn get_sidecar_deploy_ready_signals() -> Result<Vec<crate::meta::signals::data::SidecarDeployReadySignal>>;
    }

    // Generic signal methods that access self.db.conn() directly — hand-written.

    /// Check if an inode-keyed corpus signal exists (generic, type-safe).
    pub fn corpus_signal_exists<S: crate::meta::signals::store::CorpusSignalStore>(
        &self,
        inode: i64,
    ) -> bool {
        S::exists(self.db.conn(), inode).unwrap_or(false)
    }

    /// Count signals in a corpus signal table (generic, type-safe).
    pub fn corpus_signal_count<S: crate::meta::signals::store::CorpusSignalStore>(&self) -> usize {
        S::count(self.db.conn()).unwrap_or(0)
    }

    /// Query all inodes that have signals in a corpus signal table (generic, type-safe).
    pub fn corpus_signal_all_inodes<S: crate::meta::signals::store::CorpusSignalStore>(
        &self,
    ) -> Result<Vec<i64>> {
        Ok(S::all_inodes(self.db.conn())?)
    }

    /// Query all keys for an aggregate signal type (generic, type-safe).
    pub fn aggregate_signal_keys<S: crate::meta::signals::store::AggregateSignalStore>(
        &self,
    ) -> Result<Vec<String>> {
        S::query_keys(self.db.conn()).map_err(|e| {
            anyhow::anyhow!("Failed to query signal keys for {}: {}", S::TABLE_NAME, e)
        })
    }

    /// Query key→data_hash map for an aggregate BLOB signal type.
    pub fn aggregate_signal_key_hashes<S: crate::meta::signals::store::AggregateSignalStore>(
        &self,
    ) -> Result<std::collections::HashMap<String, i64>> {
        S::query_key_hashes(self.db.conn()).map_err(|e| {
            anyhow::anyhow!(
                "Failed to query signal key hashes for {}: {}",
                S::TABLE_NAME,
                e
            )
        })
    }

    /// Query inode→data_hash map for a corpus BLOB signal type.
    pub fn corpus_signal_inode_hashes<S: crate::meta::signals::store::CorpusSignalStore>(
        &self,
    ) -> Result<std::collections::HashMap<i64, i64>> {
        S::query_inode_hashes(self.db.conn()).map_err(|e| {
            anyhow::anyhow!(
                "Failed to query signal inode hashes for {}: {}",
                S::TABLE_NAME,
                e
            )
        })
    }

    /// Query the data_hash for a single inode in a corpus signal table.
    pub fn corpus_signal_inode_hash<S: crate::meta::signals::store::CorpusSignalStore>(
        &self,
        inode: i64,
    ) -> Option<i64> {
        S::query_inode_hash(self.db.conn(), inode).ok().flatten()
    }

    /// Check if a TypedSignalWrite already exists in its typed table.
    pub fn signal_exists(&self, signal: &crate::meta::signals::registry::TypedSignalWrite) -> bool {
        signal.exists(self.db.conn())
    }

    // =========================================================================
    // Zone-Generic Queries
    // =========================================================================

    /// Get all audio file inodes mapped to their paths for a zone.
    pub fn get_all_inodes<Z: crate::zones::AudioZone>(
        &self,
    ) -> Result<std::collections::HashMap<i64, String>> {
        self.db.get_all_inodes::<Z>()
    }

    /// Get all file-presence signal inodes with their paths for a zone.
    pub fn get_file_presence_inodes<Z: crate::zones::AudioZone>(
        &self,
    ) -> Result<std::collections::HashMap<i64, String>> {
        self.db.get_file_presence_inodes::<Z>()
    }

    /// Get distinct tag values with file counts for a tagged zone.
    pub fn get_distinct_tag_values_for<Z: crate::zones::TaggedZone>(
        &self,
        tag_name: &str,
    ) -> Result<Vec<(String, usize)>> {
        self.db.get_distinct_tag_values_for::<Z>(tag_name)
    }

    /// Get inodes matching tag values for a tagged zone (normalized tag name matching).
    pub fn get_inodes_for_tag_values_in<Z: crate::zones::TaggedZone>(
        &self,
        tag_name: &str,
        values: &[&str],
    ) -> Result<Vec<i64>> {
        self.db.get_inodes_for_tag_values_in::<Z>(tag_name, values)
    }

    /// Get audio files with tag presence info for a tagged zone.
    #[allow(clippy::type_complexity)]
    pub fn get_audio_files_with_tag_presence_for<Z: crate::zones::TaggedZone>(
        &self,
    ) -> Result<Vec<(i64, String, Option<String>, Option<String>, Option<String>, Option<String>)>>
    {
        self.db.get_audio_files_with_tag_presence_for::<Z>()
    }

    /// Get the file path for a single inode in the given zone.
    pub fn get_path_for_inode<Z: crate::zones::AudioZone>(
        &self,
        inode: i64,
    ) -> Result<Option<String>> {
        self.db.get_path_for_inode::<Z>(inode)
    }

    /// Get all tags for an audio file in the given tagged zone.
    pub fn get_tags<Z: crate::zones::TaggedZone>(
        &self,
        inode: i64,
    ) -> Result<Vec<super::types::AudioTag>> {
        self.db.get_tags::<Z>(inode)
    }

    /// Get all unindexed signal (inode, path) pairs for a zone.
    pub fn get_unindexed_signals_for<Z: crate::zones::AudioZone>(
        &self,
    ) -> Result<Vec<(i64, String)>> {
        self.db.get_unindexed_signals_for::<Z>()
    }

    // =========================================================================
    // OOB / Tag Mismatch Queries
    // =========================================================================

    delegate_read! {
        fn get_oob_sync_files() -> Result<Vec<crate::meta::views::OobSyncFile>>;
        fn get_oob_files_bucketed() -> Result<Vec<crate::meta::views::BucketedOobFile>>;
        fn get_moved_files() -> Result<Vec<crate::meta::views::MovedFileInfo>>;
    }

    // =========================================================================
    // Deploy Queries
    // =========================================================================

    delegate_read! {
        fn get_deploy_ready_files() -> Result<Vec<crate::meta::views::DeploySignalFile>>;
        fn get_deployed_healthy_files() -> Result<Vec<crate::meta::views::DeploySignalFile>>;
        fn get_library_stale_files() -> Result<Vec<crate::meta::views::StaleSignalFile>>;
        fn get_library_leftover_files() -> Result<Vec<crate::meta::views::LeftoverSignalFile>>;
        fn get_deploy_conflict_groups() -> Result<Vec<crate::meta::views::ConflictGroup>>;
        fn get_sidecar_conflict_groups() -> Result<Vec<crate::meta::views::SidecarConflictGroup>>;
        fn get_missing_file_paths() -> Result<Vec<String>>;
        fn get_corrupt_file_paths() -> Result<Vec<String>>;
        fn get_shit_format_files() -> Result<Vec<(i64, String, String)>>;
        fn get_shit_format_counts_by_type() -> Result<Vec<(String, i64)>>;
        fn get_subpar_duplicate_files() -> Result<Vec<crate::meta::views::SubparDuplicateEntry>>;
        fn get_inbox_corpus_match_entries(bitrate_fuzz_percent: f64) -> Result<Vec<crate::meta::views::InboxCorpusMatchEntry>>;
        fn get_organizable_inbox_files() -> Result<Vec<(i64, String)>>;
    }

    // =========================================================================
    // Library File Queries
    // =========================================================================

    delegate_read! {
        fn get_all_library_files() -> Result<Vec<library_scan::LibraryScanEntry>>;
        fn get_library_files(library_name: &str) -> Result<Vec<library_scan::LibraryScanEntry>>;
    }

    // =========================================================================
    // File Entry Queries
    // =========================================================================

    delegate_read! {
        fn get_file_entry_by_path(path: &str, zone: &str) -> Result<Option<super::types::FileEntry>>;
        fn get_file_mtime_batch(source: super::types::Zone, inodes: &[i64]) -> Result<std::collections::HashMap<i64, (i64, i64)>>;
        fn get_all_file_mtimes(zone: super::types::Zone) -> Result<std::collections::HashMap<i64, (i64, i64)>>;
        fn get_file_zone_and_path_by_inode(inode: i64) -> Result<Option<(String, String)>>;
        fn get_file_paths_batch(source: super::types::Zone, inodes: &[i64]) -> Result<std::collections::HashMap<i64, String>>;
        fn get_duplicate_inode_groups() -> Result<Vec<(i64, String)>>;
        fn get_compilation_albums() -> Result<std::collections::HashSet<String>>;
        fn get_all_tags_ordered() -> Result<Vec<(i64, String, String)>>;
        fn get_indexed_corpus_directories() -> Result<Vec<(std::path::PathBuf, i64)>>;
        fn get_missing_directory_paths() -> Result<Vec<String>>;
        fn get_album_values_with_inodes() -> Result<Vec<(i64, String)>>;
        #[allow(clippy::type_complexity)]
        fn get_album_data_for_collision_detection() -> Result<Vec<(String, String, String, String, String, String)>>;
        #[allow(clippy::type_complexity)]
        fn get_album_artist_data() -> Result<Vec<(i64, String, String, String, String, String, String, String)>>;
        fn directory_entry_fresh(zone: &str, inode: i64, mtime_secs: i64, mtime_nanos: i64) -> bool;
    }

    // =========================================================================
    // CanonicalTag Queries
    // =========================================================================

    delegate_read! {
        fn is_canonical_tag(tag_name: &str, tag_value: &str) -> Result<bool>;
        fn get_inodes_with_compound_value(tag_name: &str, compound_value: &str) -> Result<Vec<i64>>;
    }

    // =========================================================================
    // ExpectedOverlap / ExpectedDuplicate Queries
    // =========================================================================

    delegate_read! {
        fn is_expected_overlap(pair_key: &str) -> Result<bool>;
        fn is_expected_duplicate(fingerprint_key: &str) -> Result<bool>;
    }

    // =========================================================================
    // External Match / MusicBrainz Cache Queries
    // =========================================================================

    delegate_read! {
        fn get_external_matches_data() -> Result<crate::meta::views::ExternalMatchesData>;
        fn get_external_matches_for_derivation(source_key: i64) -> Result<Vec<external::ExternalMatchRow>>;
        fn get_mb_recording_cache(recording_id: &str) -> Result<Option<(Vec<u8>, i64)>>;
        fn get_mb_artist_cache(artist_id: &str) -> Result<Option<(Vec<u8>, i64)>>;
        fn get_packing_manifest() -> Result<Vec<external::PackingManifestRow>>;
        fn get_optimal_packing_scores() -> Result<Vec<external::OptimalPackingScoreRow>>;
        fn get_mb_release_cache(release_id: &str) -> Result<Option<(Vec<u8>, i64)>>;
        fn get_external_matches_slim(source_key: i64) -> Result<Vec<external::ExternalMatchRow>>;
        fn get_packing_candidates_for_release(release_id: &str) -> Result<Vec<external::PackingCandidateRow>>;
        fn get_packing_inode_paths() -> Result<Vec<(i64, String)>>;
        fn get_packing_knots() -> Result<Vec<crate::meta::signals::data::PackingKnotData>>;
        fn get_candidate_inode_dirs() -> Result<Vec<(i64, String, i32)>>;
        fn get_candidate_inode_recordings() -> Result<Vec<(i64, String)>>;
        fn get_release_packing_assignments() -> Result<Vec<external::PackingAssignment>>;
        fn get_unassigned_audio_in_directory(parent_dir: &str, assigned_inodes: &std::collections::HashSet<i64>) -> Result<Vec<external::UnassignedAudioFile>>;
        fn get_mb_release_cache_bulk(release_ids: &[&str]) -> Result<Vec<(String, Vec<u8>)>>;
    }

    // =========================================================================
    // Release Packing Signal Data Queries
    // =========================================================================

    delegate_read! {
        fn get_release_packing_signal_data() -> Result<Vec<(i64, String, crate::meta::signals::data::ReleasePackingData)>>;
        fn get_unmatched_corpus_track_signal_data_by_category(category: &str) -> Result<Vec<(i64, String, crate::meta::signals::data::UnmatchedCorpusTrackData)>>;
        fn get_unfilled_release_slot_signal_data() -> Result<Vec<crate::meta::signals::data::UnfilledReleaseSlotData>>;
        fn get_fingerprinted_corpus_inodes() -> Result<Vec<(i64, String)>>;
        fn get_assigned_packing_inodes() -> Result<std::collections::HashSet<i64>>;
        fn get_packed_releases_by_category(category_prefix: &str) -> Result<Vec<crate::meta::signals::data::PackedReleaseData>>;
        fn get_alternative_release_packing_data() -> Result<Vec<crate::meta::signals::data::AlternativeReleasePackingData>>;
        fn get_various_artists_override_data() -> Result<Vec<crate::meta::signals::data::VariousArtistsOverrideData>>;
        fn get_packing_assigned_paths() -> Result<std::collections::HashSet<std::path::PathBuf>>;
        fn get_packing_directory_categories() -> Result<std::collections::HashMap<std::path::PathBuf, crate::meta::signals::packing_category::PackingCategory>>;
    }

    // =========================================================================
    // Redundant / Metadata Duplicate Queries
    // =========================================================================

    delegate_read! {
        fn get_redundant_duplicate_groups() -> Result<Vec<(String, crate::meta::signals::data::RedundantDuplicateData)>>;
        fn get_metadata_duplicate_groups() -> Result<Vec<(String, crate::meta::signals::data::MetadataDuplicateData)>>;
    }

    // =========================================================================
    // Dirty Inode Queries (for incremental computations)
    // =========================================================================

    delegate_read! {
        fn is_pending_write(inode: i64) -> Result<bool>;
        fn get_dirty_inodes(computation_type: &str) -> Result<Vec<i64>>;
        fn get_corpus_inodes_with_tag_separator(tag_name: &str, separator: &str) -> Result<Vec<i64>>;
    }

    // =========================================================================
    // Library File Metadata (for reconciliation)
    // =========================================================================

    delegate_read! {
        fn get_library_file_metadata() -> Result<std::collections::HashMap<String, (i64, i64, i64, i64)>>;
    }

    // =========================================================================
    // Edit History Queries
    // =========================================================================

    delegate_read! {
        fn get_edit_sessions() -> Result<Vec<crate::meta::views::EditSessionSummary>>;
        fn get_session_edits(session_id: &str) -> Result<Vec<crate::meta::views::EditRecord>>;
        fn get_all_edit_history() -> Result<Vec<crate::meta::views::EditHistoryExportRow>>;
        fn get_session_edit_history(session_id: &str) -> Result<Vec<crate::meta::views::EditHistoryExportRow>>;
    }

    // =========================================================================
    // Health / Cache Queries
    // =========================================================================

    delegate_read! {
        fn get_insights_data() -> Result<crate::meta::views::InsightsData>;
        fn get_inbox_overview_data() -> Result<crate::meta::views::InboxOverviewData>;
        fn get_deploy_status() -> Result<crate::meta::views::DeployStatus>;
    }

    // =========================================================================
    // Image File Queries
    // =========================================================================

    delegate_read! {
        fn get_image_info_exists_batch(inodes: &[i64]) -> Result<std::collections::HashSet<i64>>;
        fn get_all_corpus_images() -> Result<Vec<files::CorpusImageEntry>>;
        fn get_any_audio_sibling_in_directory(corpus_dir: &str) -> Result<Option<(i64, String)>>;
    }

    // =========================================================================
    // Auth Queries
    // =========================================================================

    delegate_read! {
        fn user_count() -> Result<i64>;
        fn get_user_by_username(username: &str) -> Result<Option<auth::UserRow>>;
    }
}
