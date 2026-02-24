//! Database operations and queries.
//!
//! All SQLite operations are organized into domain-specific submodules:
//! - `files`: Inode-based queries for files/audio_info/corpus_tags tables
//! - `health`: Health issues, summaries
//! - `metadata`: App metadata, tag canonicalization
//! - `library_scan`: Library scanning state

pub mod files;
pub mod external;
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

    /// Get all audio files for a source.
    pub fn get_all_audio_files(&self, source: super::types::Zone) -> Result<Vec<super::types::AudioFile>> {
        self.db.get_all_audio_files(source)
    }

    /// Get an audio file by path.
    pub fn get_audio_file_by_path(&self, path: &str) -> Result<Option<super::types::AudioFile>> {
        self.db.get_audio_file_by_path(path)
    }

    /// Get an audio file by inode from a specific source.
    pub fn get_audio_file_by_inode(&self, inode: i64, source: super::types::Zone) -> Result<Option<super::types::AudioFile>> {
        self.db.get_audio_file_by_inode(inode, source)
    }

    /// Get multiple audio files by their inodes from a specific source.
    pub fn get_audio_files_by_inodes(&self, inodes: &[i64], source: super::types::Zone) -> Result<Vec<super::types::AudioFile>> {
        self.db.get_audio_files_by_inodes(inodes, source)
    }

    /// Get audio files by path prefix (directory query).
    pub fn get_audio_files_by_path_prefix(&self, path_prefix: &str) -> Result<Vec<super::types::AudioFile>> {
        self.db.get_audio_files_by_path_prefix(path_prefix)
    }

    /// Get audio files in a directory for tag editing.
    pub fn get_audio_files_for_tag_editing(&self, dir_path: &std::path::Path) -> Result<Vec<super::types::AudioFile>> {
        self.db.get_audio_files_for_tag_editing(dir_path)
    }

    /// Get all audio files with their tags (for search functionality).
    pub fn get_all_audio_files_with_tags(&self, source: super::types::Zone) -> Result<Vec<(super::types::AudioFile, std::collections::HashMap<String, Vec<String>>)>> {
        self.db.get_all_audio_files_with_tags(source)
    }

    /// Get tags for an audio file by inode (corpus only).
    pub fn get_corpus_tags(&self, inode: i64) -> Result<Vec<super::types::AudioTag>> {
        self.db.get_corpus_tags(inode)
    }

    /// Get tags for an audio file by inode, dispatching to the correct tag table for the zone.
    pub fn get_tags_for_zone(&self, inode: i64, zone: super::types::Zone) -> Result<Vec<super::types::AudioTag>> {
        self.db.get_tags_for_zone(inode, zone)
    }

    // =========================================================================
    // Signal Queries
    // =========================================================================

    /// Count total signals across all typed tables.
    pub fn count_all_signals(&self) -> usize {
        self.db.count_all_signals()
    }

    // --- Typed signal queries (no JSON) ---

    pub fn get_unindexed_file_signals(&self) -> Result<Vec<crate::meta::signals::data::UnindexedFileSignal>> {
        self.db.get_unindexed_file_signals()
    }

    pub fn get_healthy_file_signals(&self) -> Result<Vec<crate::meta::signals::data::HealthyFileSignal>> {
        self.db.get_healthy_file_signals()
    }

    pub fn get_tag_canonicity_signal(&self, key: &str) -> Result<Option<crate::meta::signals::data::TagCanonicitySignal>> {
        self.db.get_tag_canonicity_signal(key)
    }

    pub fn get_inconsistent_album_artist_signal(&self, key: &str) -> Result<Option<crate::meta::signals::data::InconsistentAlbumArtistSignal>> {
        self.db.get_inconsistent_album_artist_signal(key)
    }

    pub fn get_compound_tag_signal(&self, inode: i64) -> Result<Option<crate::meta::signals::data::CompoundTagSignal>> {
        self.db.get_compound_tag_signal(inode)
    }

    pub fn get_inbox_compound_tag_signal(&self, inode: i64) -> Result<Option<crate::meta::signals::data::InboxCompoundTagSignal>> {
        self.db.get_inbox_compound_tag_signal(inode)
    }

    pub fn get_cross_source_overlap_signals(&self) -> Result<Vec<crate::meta::signals::data::CrossSourceOverlapSignal>> {
        self.db.get_cross_source_overlap_signals()
    }

    pub fn get_fingerprint_overlap_signals(&self) -> Result<Vec<crate::meta::signals::data::FingerprintOverlapSignal>> {
        self.db.get_fingerprint_overlap_signals()
    }

    pub fn get_missing_tag_signals(&self) -> Result<Vec<crate::meta::signals::data::MissingTagSignal>> {
        self.db.get_missing_tag_signals()
    }

    pub fn get_missing_album_single_signals(&self) -> Result<Vec<crate::meta::signals::data::MissingAlbumSingleSignal>> {
        self.db.get_missing_album_single_signals()
    }

    pub fn get_inbox_tag_canonicity_signal(&self, key: &str) -> Result<Option<crate::meta::signals::data::InboxTagCanonicitySignal>> {
        self.db.get_inbox_tag_canonicity_signal(key)
    }

    /// Check if an inode-keyed corpus signal exists (generic, type-safe).
    pub fn corpus_signal_exists<S: crate::meta::signals::store::CorpusSignalStore>(&self, inode: i64) -> bool {
        S::exists(self.db.conn(), inode).unwrap_or(false)
    }

    /// Query all inodes that have signals in a corpus signal table (generic, type-safe).
    pub fn corpus_signal_all_inodes<S: crate::meta::signals::store::CorpusSignalStore>(&self) -> Result<Vec<i64>> {
        Ok(S::all_inodes(self.db.conn())?)
    }

    /// Query all keys for an aggregate signal type (generic, type-safe).
    pub fn aggregate_signal_keys<S: crate::meta::signals::store::AggregateSignalStore>(&self) -> Result<Vec<String>> {
        S::query_keys(self.db.conn())
            .map_err(|e| anyhow::anyhow!("Failed to query signal keys for {}: {}", S::TABLE_NAME, e))
    }

    /// Query key→data_hash map for an aggregate BLOB signal type.
    pub fn aggregate_signal_key_hashes<S: crate::meta::signals::store::AggregateSignalStore>(&self) -> Result<std::collections::HashMap<String, i64>> {
        S::query_key_hashes(self.db.conn())
            .map_err(|e| anyhow::anyhow!("Failed to query signal key hashes for {}: {}", S::TABLE_NAME, e))
    }

    /// Query inode→data_hash map for a corpus BLOB signal type.
    pub fn corpus_signal_inode_hashes<S: crate::meta::signals::store::CorpusSignalStore>(&self) -> Result<std::collections::HashMap<i64, i64>> {
        S::query_inode_hashes(self.db.conn())
            .map_err(|e| anyhow::anyhow!("Failed to query signal inode hashes for {}: {}", S::TABLE_NAME, e))
    }

    /// Check if a TypedSignalWrite already exists in its typed table.
    pub fn signal_exists(&self, signal: &crate::meta::signals::data::TypedSignalWrite) -> bool {
        signal.exists(self.db.conn())
    }

    /// Get compound tag signal groups aggregated by (tag_name, compound_value).
    pub fn get_compound_signal_groups_by_safety(
        &self,
        safe_only: bool,
        tag_filter: Option<&str>,
    ) -> Result<Vec<crate::meta::signals::data::CompoundGroup>> {
        self.db.get_compound_signal_groups_by_safety(safe_only, tag_filter)
    }

    /// Get inbox compound tag signal groups aggregated by (tag_name, compound_value).
    pub fn get_inbox_compound_signal_groups(&self) -> Result<Vec<crate::meta::signals::data::CompoundGroup>> {
        self.db.get_inbox_compound_signal_groups()
    }

    // =========================================================================
    // OOB / Tag Mismatch Queries
    // =========================================================================

    /// Get files with OOB tag sync issues.
    pub fn get_oob_sync_files(&self) -> Result<Vec<crate::meta::views::OobSyncFile>> {
        self.db.get_oob_sync_files()
    }

    /// Get OOB files bucketed by conflict type.
    pub fn get_oob_files_bucketed(&self) -> Result<Vec<crate::meta::views::BucketedOobFile>> {
        self.db.get_oob_files_bucketed()
    }

    /// Get files that have been moved (same inode, different path).
    pub fn get_moved_files(&self) -> Result<Vec<crate::meta::views::MovedFileInfo>> {
        self.db.get_moved_files()
    }

    // =========================================================================
    // Deploy Queries
    // =========================================================================

    /// Get files ready for deployment.
    pub fn get_deploy_ready_files(&self) -> Result<Vec<crate::meta::views::DeploySignalFile>> {
        self.db.get_deploy_ready_files()
    }

    /// Get healthy deployed files.
    pub fn get_deployed_healthy_files(&self) -> Result<Vec<crate::meta::views::DeploySignalFile>> {
        self.db.get_deployed_healthy_files()
    }

    /// Get stale library files.
    pub fn get_library_stale_files(&self) -> Result<Vec<crate::meta::views::StaleSignalFile>> {
        self.db.get_library_stale_files()
    }

    /// Get leftover library files.
    pub fn get_library_leftover_files(&self) -> Result<Vec<crate::meta::views::LeftoverSignalFile>> {
        self.db.get_library_leftover_files()
    }

    /// Get deploy conflict groups.
    pub fn get_deploy_conflict_groups(&self) -> Result<Vec<crate::meta::views::ConflictGroup>> {
        self.db.get_deploy_conflict_groups()
    }

    /// Get paths of missing files.
    pub fn get_missing_file_paths(&self) -> Result<Vec<String>> {
        self.db.get_missing_file_paths()
    }

    /// Get paths of corrupt files.
    pub fn get_corrupt_file_paths(&self) -> Result<Vec<String>> {
        self.db.get_corrupt_file_paths()
    }

    /// Get shit format files (inode, signal_path, file_type).
    pub fn get_shit_format_files(&self) -> Result<Vec<(i64, String, String)>> {
        self.db.get_shit_format_files()
    }

    /// Get counts of shit format files grouped by file type.
    pub fn get_shit_format_counts_by_type(&self) -> Result<Vec<(String, i64)>> {
        self.db.get_shit_format_counts_by_type()
    }

    /// Get subpar duplicate files with metadata.
    pub fn get_subpar_duplicate_files(&self) -> Result<Vec<crate::meta::views::SubparDuplicateEntry>> {
        self.db.get_subpar_duplicate_files()
    }

    /// Get inbox corpus match entries with quality classification.
    pub fn get_inbox_corpus_match_entries(&self, bitrate_fuzz_percent: f64) -> Result<Vec<crate::meta::views::InboxCorpusMatchEntry>> {
        self.db.get_inbox_corpus_match_entries(bitrate_fuzz_percent)
    }

    /// Get inbox files eligible for organizing into the corpus.
    pub fn get_organizable_inbox_files(&self) -> Result<Vec<(i64, String)>> {
        self.db.get_organizable_inbox_files()
    }

    /// Get embeddable album art signals with deserialized data.
    pub fn get_embeddable_album_art_signals(&self) -> Result<Vec<crate::meta::signals::data::EmbeddableAlbumArtSignal>> {
        self.db.get_embeddable_album_art_signals()
    }

    /// Get corpus audio files without embedded pictures.
    pub fn get_artless_corpus_files(&self) -> Result<Vec<(i64, String)>> {
        self.db.get_artless_corpus_files()
    }

    // =========================================================================
    // Library File Queries
    // =========================================================================

    /// Get all library files.
    pub fn get_all_library_files(&self) -> Result<Vec<library_scan::LibraryScanEntry>> {
        self.db.get_all_library_files()
    }

    // =========================================================================
    // File Entry Queries
    // =========================================================================

    /// Get a file entry by path for a specific zone (without requiring audio_info).
    ///
    /// Use this for files that may not have been successfully indexed.
    pub fn get_file_entry_by_path(&self, path: &str, zone: &str) -> Result<Option<super::types::FileEntry>> {
        self.db.get_file_entry_by_path(path, zone)
    }

    /// Get mtime info for files by inode (for incremental scanning).
    pub fn get_file_mtime_batch(
        &self,
        source: super::types::Zone,
        inodes: &[i64],
    ) -> Result<std::collections::HashMap<i64, (i64, i64)>> {
        self.db.get_file_mtime_batch(source, inodes)
    }

    /// Look up the zone and path for an inode in corpus/inbox (excludes library).
    pub fn get_file_zone_and_path_by_inode(&self, inode: i64) -> Result<Option<(String, String)>> {
        self.db.get_file_zone_and_path_by_inode(inode)
    }

    /// Get paths for files by inode (for move detection).
    pub fn get_file_paths_batch(
        &self,
        source: super::types::Zone,
        inodes: &[i64],
    ) -> Result<std::collections::HashMap<i64, String>> {
        self.db.get_file_paths_batch(source, inodes)
    }

    /// Get inode groups with duplicates (multiple paths for same inode).
    pub fn get_duplicate_inode_groups(&self) -> Result<Vec<(i64, String)>> {
        self.db.get_duplicate_inode_groups()
    }

    /// Get audio files with their present tag names (for missing tag detection).
    #[allow(clippy::type_complexity)]
    pub fn get_audio_files_with_tag_presence(&self) -> Result<Vec<(i64, String, Option<String>, Option<String>, Option<String>, Option<String>)>> {
        self.db.get_audio_files_with_tag_presence()
    }

    /// Get inbox audio files with their present tag names (for inbox missing tag detection).
    #[allow(clippy::type_complexity)]
    pub fn get_inbox_audio_files_with_tag_presence(&self) -> Result<Vec<(i64, String, Option<String>, Option<String>, Option<String>, Option<String>)>> {
        self.db.get_inbox_audio_files_with_tag_presence()
    }

    /// Get the inbox path for a single inode.
    pub fn get_inbox_path_for_inode(&self, inode: i64) -> Result<Option<String>> {
        self.db.get_inbox_path_for_inode(inode)
    }

    /// Get albums that are compilations (more than one distinct ARTIST value).
    pub fn get_compilation_albums(&self) -> Result<std::collections::HashSet<String>> {
        self.db.get_compilation_albums()
    }

    /// Get inodes that have any of the given tag values for a specific tag name.
    pub fn get_inodes_for_tag_values(&self, tag_name: &str, values: &[&str]) -> Result<Vec<i64>> {
        self.db.get_inodes_for_tag_values(tag_name, values)
    }

    /// Get all corpus audio file inodes mapped to their paths.
    pub fn get_all_corpus_inodes(&self) -> Result<std::collections::HashMap<i64, String>> {
        self.db.get_all_corpus_inodes()
    }

    pub fn get_all_inbox_inodes(&self) -> Result<std::collections::HashMap<i64, String>> {
        self.db.get_all_inbox_inodes()
    }

    /// Get all FileInCorpus signal inodes with their paths.
    pub fn get_file_in_corpus_inodes(&self) -> Result<std::collections::HashMap<i64, String>> {
        self.db.get_file_in_corpus_inodes()
    }

    pub fn get_file_in_inbox_inodes(&self) -> Result<std::collections::HashMap<i64, String>> {
        self.db.get_file_in_inbox_inodes()
    }

    /// Get all inbox unindexed files as (inode, path) pairs.
    pub fn get_inbox_unindexed_files(&self) -> Result<Vec<(i64, String)>> {
        self.db.get_inbox_unindexed_files()
    }

    /// Check whether an audio file has embedded pictures.
    pub fn get_has_pictures(&self, inode: i64) -> Result<bool> {
        self.db.get_has_pictures(inode)
    }

    /// Get the corpus path for a single inode.
    pub fn get_corpus_path_for_inode(&self, inode: i64) -> Result<Option<String>> {
        self.db.get_corpus_path_for_inode(inode)
    }

    /// Get all tags ordered by inode and tag name (for metadata duplicate detection).
    pub fn get_all_tags_ordered(&self) -> Result<Vec<(i64, String, String)>> {
        self.db.get_all_tags_ordered()
    }

    /// Get indexed corpus directories with their inodes.
    pub fn get_indexed_corpus_directories(&self) -> Result<Vec<(std::path::PathBuf, i64)>> {
        self.db.get_indexed_corpus_directories()
    }

    /// Get missing directory signal paths (for UI resolution modal).
    pub fn get_missing_directory_paths(&self) -> Result<Vec<String>> {
        self.db.get_missing_directory_paths()
    }

    /// Get all files for a library (for DeriveDeployHealthSignals).
    pub fn get_library_files(&self, library_name: &str) -> Result<Vec<library_scan::LibraryScanEntry>> {
        self.db.get_library_files(library_name)
    }

    /// Get audio info for a track by inode.
    pub fn get_audio_info(&self, inode: i64) -> Result<Option<super::types::AudioInfo>> {
        self.db.get_audio_info(inode)
    }

    /// Query distinct tag values with file counts from corpus_tags table.
    pub fn get_distinct_tag_values(&self, tag_name: &str) -> Result<Vec<(String, usize)>> {
        self.db.get_distinct_tag_values(tag_name)
    }

    /// Query distinct tag values with file counts from inbox_tags table.
    pub fn get_distinct_inbox_tag_values(&self, tag_name: &str) -> Result<Vec<(String, usize)>> {
        self.db.get_distinct_inbox_tag_values(tag_name)
    }

    /// Get inbox inodes that have any of the given tag values for a specific tag name.
    pub fn get_inbox_inodes_for_tag_values(&self, tag_name: &str, values: &[&str]) -> Result<Vec<i64>> {
        self.db.get_inbox_inodes_for_tag_values(tag_name, values)
    }

    /// Get ALBUM tag values with their inodes from both corpus and inbox.
    pub fn get_album_values_with_inodes(&self) -> Result<Vec<(i64, String)>> {
        self.db.get_album_values_with_inodes()
    }

    /// Query album data with artist context for collision detection.
    #[allow(clippy::type_complexity)]
    pub fn get_album_data_for_collision_detection(&self) -> Result<Vec<(String, String, String, String, String, String)>> {
        self.db.get_album_data_for_collision_detection()
    }

    /// Get album artist data for inconsistency detection.
    #[allow(clippy::type_complexity)]
    pub fn get_album_artist_data(&self) -> Result<Vec<(i64, String, String, String, String, String, String, String)>> {
        self.db.get_album_artist_data()
    }

    // =========================================================================
    // CanonicalTag Queries
    // =========================================================================

    /// Check if a CanonicalTag signal exists for this tag_name:tag_value.
    pub fn is_canonical_tag(&self, tag_name: &str, tag_value: &str) -> Result<bool> {
        self.db.is_canonical_tag(tag_name, tag_value)
    }

    /// Get all inodes that have a CompoundTag signal containing a specific compound value.
    pub fn get_inodes_with_compound_value(&self, tag_name: &str, compound_value: &str) -> Result<Vec<i64>> {
        self.db.get_inodes_with_compound_value(tag_name, compound_value)
    }

    // =========================================================================
    // ExpectedOverlap Queries
    // =========================================================================

    /// Check if an ExpectedOverlap signal exists for this source pair key.
    pub fn is_expected_overlap(&self, pair_key: &str) -> Result<bool> {
        self.db.is_expected_overlap(pair_key)
    }

    // =========================================================================
    // ExpectedDuplicate Queries
    // =========================================================================

    /// Check if an ExpectedDuplicate signal exists for this fingerprint key.
    pub fn is_expected_duplicate(&self, fingerprint_key: &str) -> Result<bool> {
        self.db.is_expected_duplicate(fingerprint_key)
    }

    // =========================================================================
    // External Match Queries
    // =========================================================================

    /// Get external matches data bucketed for the External Matches lateral view.
    pub fn get_external_matches_data(&self) -> Result<crate::meta::views::ExternalMatchesData> {
        self.db.get_external_matches_data()
    }

    /// Get external matches for corpus files, for signal derivation.
    pub fn get_external_matches_for_derivation(
        &self,
        source_key: i64,
    ) -> Result<Vec<external::ExternalMatchRow>> {
        self.db.get_external_matches_for_derivation(source_key)
    }

    // =========================================================================
    // Redundant / Metadata Duplicate Queries
    // =========================================================================

    /// Get all redundant duplicate groups with deserialized data.
    pub fn get_redundant_duplicate_groups(&self) -> Result<Vec<(String, crate::meta::signals::data::RedundantDuplicateData)>> {
        self.db.get_redundant_duplicate_groups()
    }

    /// Get all metadata duplicate groups with deserialized data.
    pub fn get_metadata_duplicate_groups(&self) -> Result<Vec<(String, crate::meta::signals::data::MetadataDuplicateData)>> {
        self.db.get_metadata_duplicate_groups()
    }

    // =========================================================================
    // Dirty Inode Queries (for incremental computations)
    // =========================================================================

    /// Get all inodes marked dirty for a specific computation type.
    ///
    /// Used by per-inode computations (e.g., compound tag detection) to query
    /// only the inodes that need reprocessing instead of the entire corpus.
    pub fn get_dirty_inodes(&self, computation_type: &str) -> Result<Vec<i64>> {
        self.db.get_dirty_inodes(computation_type)
    }

    // =========================================================================
    // Directory Entry Freshness Check
    // =========================================================================

    /// Check if a directory entry already matches the given metadata.
    ///
    /// Used by `index_directory()` to skip unconditional writes.
    pub fn directory_entry_fresh(
        &self,
        zone: &str,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
    ) -> bool {
        self.db.directory_entry_fresh(zone, inode, mtime_secs, mtime_nanos)
    }

    // =========================================================================
    // Library File Metadata (for reconciliation)
    // =========================================================================

    /// Get all library file metadata for reconciliation.
    ///
    /// Returns a map of stored_path → (inode, mtime_secs, mtime_nanos, file_size).
    pub fn get_library_file_metadata(&self) -> Result<std::collections::HashMap<String, (i64, i64, i64, i64)>> {
        self.db.get_library_file_metadata()
    }

    // =========================================================================
    // Edit History Queries
    // =========================================================================

    /// Get all edit sessions, most recent first.
    pub fn get_edit_sessions(&self) -> Result<Vec<crate::meta::views::EditSessionSummary>> {
        self.db.get_edit_sessions()
    }

    /// Get all edits within a single session.
    pub fn get_session_edits(&self, session_id: &str) -> Result<Vec<crate::meta::views::EditRecord>> {
        self.db.get_session_edits(session_id)
    }

    /// Get all edit history rows for export (includes session_id).
    pub fn get_all_edit_history(&self) -> Result<Vec<crate::meta::views::EditHistoryExportRow>> {
        self.db.get_all_edit_history()
    }

    /// Get edit history rows for a single session, for export.
    pub fn get_session_edit_history(&self, session_id: &str) -> Result<Vec<crate::meta::views::EditHistoryExportRow>> {
        self.db.get_session_edit_history(session_id)
    }

    // =========================================================================
    // Health / Cache Queries
    // =========================================================================

    /// Get insights data for the health view.
    pub fn get_insights_data(&self) -> Result<crate::meta::views::InsightsData> {
        self.db.get_insights_data()
    }

    /// Get inbox overview data.
    pub fn get_inbox_overview_data(&self) -> Result<crate::meta::views::InboxOverviewData> {
        self.db.get_inbox_overview_data()
    }

    /// Get deploy status.
    pub fn get_deploy_status(&self) -> Result<crate::meta::views::DeployStatus> {
        self.db.get_deploy_status()
    }

    // =========================================================================
    // Schema / Migration Queries
    // =========================================================================

    /// Get the current database schema version.
    pub fn get_schema_version(&self) -> Result<u32> {
        self.db.get_schema_version()
    }
}
