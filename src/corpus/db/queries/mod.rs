//! Database operations and queries.
//!
//! All SQLite operations are organized into domain-specific submodules:
//! - `files`: Low-level inode-based queries for files/audio_info/corpus_tags tables
//! - `tracks`: Compatibility layer returning Track/TrackTag types (queries new schema)
//! - `health`: Health issues, summaries
//! - `metadata`: App metadata, tag canonicalization
//! - `library_scan`: Library scanning state

mod deployment;
pub mod files;
mod health;
mod library_scan;
mod metadata;
pub mod tracks;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
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
/// - `db_thread::signal_sender()` for writes
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
    /// - `db_thread::spawn()` - the one true write connection
    /// - `startup/first_time_setup.rs` - initial database creation
    /// - `startup/migrations.rs` - pre-Witch schema migrations
    ///
    /// If you're trying to call this elsewhere, you're violating architecture.
    /// - For writes: Use `db_thread::signal_sender()`
    /// - For reads: Use `witch.read_db()` (returns `ReadOnlyDb`)
    pub fn open(path: &Path) -> Result<Self> {
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
        db.initialize_schema()?;
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

    fn initialize_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            -- =================================================================
            -- Files Table (all paths - files AND directories)
            -- =================================================================
            -- Unified table for all path manifestations across corpus, inbox, and library.
            -- Multiple rows can share the same inode (hard links across corpus + library).
            -- Directories are tracked for scan optimization (skip unchanged dirs).
            CREATE TABLE IF NOT EXISTS files (
                inode INTEGER NOT NULL,
                source TEXT NOT NULL,           -- 'inbox', 'corpus', 'library'
                path TEXT NOT NULL,             -- relative path (library paths include library name prefix)
                is_dir INTEGER NOT NULL,        -- 1 = directory, 0 = file
                mtime_secs INTEGER NOT NULL,    -- filesystem mtime (same across hard links)
                mtime_nanos INTEGER NOT NULL,
                file_size INTEGER NOT NULL,     -- (same across hard links)
                scanned_at INTEGER NOT NULL,
                PRIMARY KEY (inode, source, path)
            );

            CREATE INDEX IF NOT EXISTS idx_files_source ON files(source);
            CREATE INDEX IF NOT EXISTS idx_files_inode ON files(inode);
            CREATE INDEX IF NOT EXISTS idx_files_is_dir ON files(is_dir);
            CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);

            -- =================================================================
            -- Audio Info Table (audio files ONLY - not directories)
            -- =================================================================
            -- Audio-specific metadata for audio files. Only audio file inodes
            -- have entries here. Directories do not.
            CREATE TABLE IF NOT EXISTS audio_info (
                inode INTEGER PRIMARY KEY,
                file_type TEXT NOT NULL,        -- flac, mp3, opus, etc.
                duration_ms INTEGER,
                bitrate_kbps INTEGER,
                sample_rate INTEGER,
                fingerprint BLOB,
                needs_tag_flush INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_audio_info_fingerprint ON audio_info(fingerprint);
            CREATE INDEX IF NOT EXISTS idx_audio_info_duration ON audio_info(duration_ms);

            -- =================================================================
            -- Corpus Tags Table
            -- =================================================================
            -- Tags for corpus audio files. Supports multi-value tags.
            CREATE TABLE IF NOT EXISTS corpus_tags (
                inode INTEGER NOT NULL REFERENCES audio_info(inode) ON DELETE CASCADE,
                tag_name TEXT NOT NULL,
                tag_value TEXT NOT NULL,
                PRIMARY KEY (inode, tag_name, tag_value)
            );

            CREATE INDEX IF NOT EXISTS idx_corpus_tags_inode ON corpus_tags(inode);
            CREATE INDEX IF NOT EXISTS idx_corpus_tags_name ON corpus_tags(tag_name);
            CREATE INDEX IF NOT EXISTS idx_corpus_tags_name_value ON corpus_tags(tag_name, tag_value);
            CREATE INDEX IF NOT EXISTS idx_corpus_tags_name_value_lower ON corpus_tags(tag_name, LOWER(tag_value));

            -- =================================================================
            -- Inbox Tags Table (future use)
            -- =================================================================
            -- Tags for inbox audio files. Completely separate from corpus_tags.
            -- Assimilation = move rows from inbox_tags → corpus_tags.
            CREATE TABLE IF NOT EXISTS inbox_tags (
                inode INTEGER NOT NULL REFERENCES audio_info(inode) ON DELETE CASCADE,
                tag_name TEXT NOT NULL,
                tag_value TEXT NOT NULL,
                PRIMARY KEY (inode, tag_name, tag_value)
            );

            CREATE INDEX IF NOT EXISTS idx_inbox_tags_inode ON inbox_tags(inode);
            CREATE INDEX IF NOT EXISTS idx_inbox_tags_name ON inbox_tags(tag_name);

            -- =================================================================
            -- Tag Edit History
            -- =================================================================
            -- Uses inode as the audio file identifier (not synthetic track_id).
            -- session_id should be decision timestamp + source label for grouping.
            CREATE TABLE IF NOT EXISTS tag_edit_history (
                id INTEGER PRIMARY KEY,
                inode INTEGER NOT NULL REFERENCES audio_info(inode) ON DELETE CASCADE,
                field_name TEXT NOT NULL,
                old_value TEXT,
                new_value TEXT,
                edited_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                session_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_tag_history_inode ON tag_edit_history(inode);
            CREATE INDEX IF NOT EXISTS idx_tag_history_session ON tag_edit_history(session_id);

            -- =================================================================
            -- Corpus Health Stats (cached aggregates)
            -- =================================================================
            CREATE TABLE IF NOT EXISTS corpus_health_stats (
                id INTEGER PRIMARY KEY,
                stat_type TEXT NOT NULL,
                last_updated DATETIME DEFAULT CURRENT_TIMESTAMP,
                data_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_corpus_health_type ON corpus_health_stats(stat_type);

            -- =================================================================
            -- Signals (facts about corpus state)
            -- =================================================================
            -- Signals are created by computations and deleted when stale.
            -- Note: column names kept as issue_type/issue_key for backwards compat.
            CREATE TABLE IF NOT EXISTS signals (
                id INTEGER PRIMARY KEY,
                issue_type TEXT NOT NULL,
                issue_key TEXT NOT NULL,
                discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                metadata_json TEXT,
                UNIQUE(issue_type, issue_key)
            );
            CREATE INDEX IF NOT EXISTS idx_signals_type ON signals(issue_type);
            CREATE INDEX IF NOT EXISTS idx_signals_discovered ON signals(discovered_at);

            -- =================================================================
            -- Application Metadata (version tracking)
            -- =================================================================
            CREATE TABLE IF NOT EXISTS app_metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#
        ).context("Failed to initialize database schema")?;

        Ok(())
    }

    // =========================================================================
    // Schema Version Management
    // =========================================================================

    /// Get the current database schema version.
    ///
    /// Returns the version stored in app_metadata, or 2 (baseline) if not set.
    pub fn get_schema_version(&self) -> Result<u32> {
        let version: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM app_metadata WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .ok();

        match version {
            Some(v) => v.parse::<u32>().context("Invalid schema version in database"),
            None => {
                // No version recorded - set baseline version
                self.set_schema_version(1)?;
                Ok(1)
            }
        }
    }

    /// Set the database schema version.
    pub fn set_schema_version(&self, version: u32) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO app_metadata (key, value, updated_at) VALUES ('schema_version', ?1, datetime('now'))",
            params![version.to_string()],
        ).context("Failed to set schema version")?;
        Ok(())
    }

    /// Execute a batch of SQL statements.
    ///
    /// Used by migrations to run multiple statements atomically.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.conn
            .execute_batch(sql)
            .context("Failed to execute SQL batch")
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
/// let tracks = read_db.get_all_tracks(None)?;
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
    // =========================================================================
    // Count Queries
    // =========================================================================

    /// Get audio file count, optionally filtered by source.
    pub fn get_audio_file_count(&self, source: Option<&str>) -> Result<usize> {
        self.db.get_audio_file_count(source)
    }

    /// Get count of audio files that have fingerprints.
    pub fn get_fingerprinted_audio_file_count(&self) -> Result<i64> {
        self.db.get_fingerprinted_audio_file_count()
    }

    // =========================================================================
    // AudioFile Queries
    // =========================================================================

    /// Get all audio files for a source.
    pub fn get_all_audio_files(&self, source: super::types::FileSource) -> Result<Vec<super::types::AudioFile>> {
        self.db.get_all_audio_files(source)
    }

    /// Get an audio file by path.
    pub fn get_audio_file_by_path(&self, path: &str) -> Result<Option<super::types::AudioFile>> {
        self.db.get_audio_file_by_path(path)
    }

    /// Get an audio file by inode.
    pub fn get_audio_file_by_inode(&self, inode: i64) -> Result<Option<super::types::AudioFile>> {
        self.db.get_audio_file_by_inode(inode)
    }

    /// Get multiple audio files by their inodes.
    pub fn get_audio_files_by_inodes(&self, inodes: &[i64]) -> Result<Vec<super::types::AudioFile>> {
        self.db.get_audio_files_by_inodes(inodes)
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
    pub fn get_all_audio_files_with_tags(&self, source: super::types::FileSource) -> Result<Vec<(super::types::AudioFile, std::collections::HashMap<String, String>)>> {
        self.db.get_all_audio_files_with_tags(source)
    }

    /// Get tags for an audio file by inode.
    pub fn get_corpus_tags(&self, inode: i64) -> Result<Vec<super::types::AudioTag>> {
        self.db.get_corpus_tags(inode)
    }

    /// Get audio file counts grouped by file type.
    pub fn get_audio_type_counts(&self) -> Result<std::collections::HashMap<String, i64>> {
        self.db.get_audio_type_counts()
    }

    /// Get audio files by file types.
    /// Returns (inode, path, file_type) tuples.
    pub fn get_audio_files_by_types(&self, file_types: &[&str]) -> Result<Vec<(i64, String, String)>> {
        self.db.get_audio_files_by_types(file_types)
    }

    // =========================================================================
    // Signal Queries
    // =========================================================================

    /// Get signals, optionally filtered by type.
    pub fn get_signals(&self, issue_type: Option<super::types::SignalType>) -> Result<Vec<super::types::Signal>> {
        self.db.get_signals(issue_type)
    }

    /// Get a signal by its ID.
    pub fn get_signal_by_id(&self, signal_id: i64) -> Result<Option<super::types::Signal>> {
        self.db.get_signal_by_id(signal_id)
    }

    /// Get aggregate signals, optionally filtered by type.
    pub fn get_aggregate_signals(&self, signal_type: Option<super::types::AggregateSignalType>) -> Result<Vec<super::types::AggregateSignal>> {
        self.db.get_aggregate_signals(signal_type)
    }

    // =========================================================================
    // OOB / Tag Mismatch Queries
    // =========================================================================

    /// Get files with OOB tag sync issues.
    pub fn get_oob_sync_files(&self) -> Result<Vec<crate::corpus::db::types::OobSyncFile>> {
        self.db.get_oob_sync_files()
    }

    /// Get OOB files bucketed by conflict type.
    pub fn get_oob_files_bucketed(&self) -> Result<Vec<crate::corpus::db::types::BucketedOobFile>> {
        self.db.get_oob_files_bucketed()
    }

    /// Get files with changed inodes.
    pub fn get_inode_changed_files(&self) -> Result<Vec<crate::corpus::db::types::InodeChangedFile>> {
        self.db.get_inode_changed_files()
    }

    /// Get files that have been moved (same inode, different path).
    pub fn get_moved_files(&self) -> Result<Vec<crate::corpus::db::types::MovedFileInfo>> {
        self.db.get_moved_files()
    }

    // =========================================================================
    // Deploy Queries
    // =========================================================================

    /// Get files ready for deployment.
    pub fn get_deploy_ready_files(&self) -> Result<Vec<super::types::DeploySignalFile>> {
        self.db.get_deploy_ready_files()
    }

    /// Get healthy deployed files.
    pub fn get_deployed_healthy_files(&self) -> Result<Vec<super::types::DeploySignalFile>> {
        self.db.get_deployed_healthy_files()
    }

    /// Get stale library files.
    pub fn get_library_stale_files(&self) -> Result<Vec<super::types::StaleSignalFile>> {
        self.db.get_library_stale_files()
    }

    /// Get leftover library files.
    pub fn get_library_leftover_files(&self) -> Result<Vec<super::types::LeftoverSignalFile>> {
        self.db.get_library_leftover_files()
    }

    /// Get deploy conflict groups.
    pub fn get_deploy_conflict_groups(&self) -> Result<Vec<super::types::ConflictGroup>> {
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

    /// Get shit format files (path, file_type).
    pub fn get_shit_format_files(&self) -> Result<Vec<(String, String)>> {
        self.db.get_shit_format_files()
    }

    /// Get counts of shit format files grouped by file type.
    pub fn get_shit_format_counts_by_type(&self) -> Result<Vec<(String, i64)>> {
        self.db.get_shit_format_counts_by_type()
    }

    /// Get subpar duplicate files with metadata.
    pub fn get_subpar_duplicate_files(&self) -> Result<Vec<crate::corpus::db::types::SubparDuplicateEntry>> {
        self.db.get_subpar_duplicate_files()
    }

    // =========================================================================
    // Library File Queries
    // =========================================================================

    /// Get all library files.
    pub fn get_all_library_files(&self) -> Result<Vec<library_scan::LibraryScanEntry>> {
        self.db.get_all_library_files()
    }
}
