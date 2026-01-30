//! Database operations and queries.
//!
//! All SQLite operations are organized into domain-specific submodules:
//! - `tracks`: Track CRUD and queries by source/path/fingerprint/metadata
//! - `scan_state`: Incremental scan state tracking
//! - `deployment`: Deployment logging and library track queries
//! - `health`: Health issues, summaries, known variants
//! - `metadata`: App metadata, tag canonicalization, tag mismatches

mod deployment;
mod health;
mod library_scan;
mod metadata;
mod scan_state;
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
            -- Tracks table: file and audio waveform metadata ONLY
            -- Tag metadata is stored in track_tags table
            CREATE TABLE IF NOT EXISTS tracks (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                source TEXT NOT NULL,
                inode INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                file_type TEXT NOT NULL,
                duration_ms INTEGER,
                bitrate_kbps INTEGER,
                sample_rate INTEGER,
                fingerprint TEXT,
                scanned_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_source ON tracks(source);
            CREATE INDEX IF NOT EXISTS idx_inode ON tracks(inode);
            CREATE INDEX IF NOT EXISTS idx_duration ON tracks(duration_ms);
            CREATE INDEX IF NOT EXISTS idx_fingerprint ON tracks(fingerprint);

            -- Track tags table: all tag metadata
            -- Supports multi-value tags (same tag_name can have multiple values)
            CREATE TABLE IF NOT EXISTS track_tags (
                track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
                tag_name TEXT NOT NULL,
                tag_value TEXT NOT NULL,
                PRIMARY KEY (track_id, tag_name, tag_value)
            );
            CREATE INDEX IF NOT EXISTS idx_track_tags_track ON track_tags(track_id);
            CREATE INDEX IF NOT EXISTS idx_track_tags_name ON track_tags(tag_name);
            CREATE INDEX IF NOT EXISTS idx_track_tags_name_value ON track_tags(tag_name, tag_value);

            CREATE TABLE IF NOT EXISTS scan_history (
                id INTEGER PRIMARY KEY,
                source TEXT NOT NULL,
                file_count INTEGER NOT NULL,
                started_at DATETIME NOT NULL,
                completed_at DATETIME NOT NULL
            );

            CREATE TABLE IF NOT EXISTS scan_state (
                id INTEGER PRIMARY KEY,
                source TEXT NOT NULL,
                inode INTEGER NOT NULL,
                path TEXT NOT NULL,
                mtime_secs INTEGER NOT NULL,
                mtime_nanos INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                scanned_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(source, inode)
            );

            CREATE INDEX IF NOT EXISTS idx_scan_state_source ON scan_state(source);
            CREATE INDEX IF NOT EXISTS idx_scan_state_inode ON scan_state(inode);
            CREATE INDEX IF NOT EXISTS idx_scan_state_path ON scan_state(path);

            CREATE TABLE IF NOT EXISTS deployment_log (
                id INTEGER PRIMARY KEY,
                library_name TEXT NOT NULL,
                corpus_path TEXT NOT NULL,
                deployed_path TEXT NOT NULL,
                inode INTEGER NOT NULL,
                deployed_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_deployment_library ON deployment_log(library_name);
            CREATE INDEX IF NOT EXISTS idx_deployment_inode ON deployment_log(inode);

            CREATE TABLE IF NOT EXISTS corpus_health_stats (
                id INTEGER PRIMARY KEY,
                stat_type TEXT NOT NULL,
                last_updated DATETIME DEFAULT CURRENT_TIMESTAMP,
                data_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_corpus_health_type ON corpus_health_stats(stat_type);

            -- Legacy tables (duplicate_groups, duplicate_group_members) removed in migration v3->v4
            -- Replaced by DeployConflict health issues

            CREATE TABLE IF NOT EXISTS tag_edit_history (
                id INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                field_name TEXT NOT NULL,
                old_value TEXT,
                new_value TEXT,
                edited_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                session_id TEXT,
                FOREIGN KEY(track_id) REFERENCES tracks(id)
            );
            CREATE INDEX IF NOT EXISTS idx_tag_history_track ON tag_edit_history(track_id);
            CREATE INDEX IF NOT EXISTS idx_tag_history_session ON tag_edit_history(session_id);

            -- =================================================================
            -- Corpus Signal Tables
            -- =================================================================

            -- Signals (facts about corpus state)
            -- Signals are created by computations and deleted when stale
            -- Note: column names kept as issue_type/issue_key for backwards compat
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

            -- Known variants (legitimate re-releases, remixes, etc.)
            CREATE TABLE IF NOT EXISTS known_variants (
                id INTEGER PRIMARY KEY,
                variant_type TEXT NOT NULL,
                canonical_fingerprint TEXT NOT NULL,
                variant_fingerprint TEXT,
                canonical_track_id INTEGER,
                variant_track_id INTEGER,
                marked_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                notes TEXT,
                FOREIGN KEY(canonical_track_id) REFERENCES tracks(id) ON DELETE SET NULL,
                FOREIGN KEY(variant_track_id) REFERENCES tracks(id) ON DELETE SET NULL
            );
            CREATE INDEX IF NOT EXISTS idx_known_variants_fingerprint ON known_variants(canonical_fingerprint);
            CREATE INDEX IF NOT EXISTS idx_known_variants_type ON known_variants(variant_type);

            -- Tag mismatches: tracks where DB tags differ from on-disk tags
            -- Records pending tag flushes (DB updated but disk not yet written)
            CREATE TABLE IF NOT EXISTS tag_mismatches (
                id INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
                field TEXT NOT NULL,
                db_value TEXT,
                disk_value TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(track_id, field)
            );
            CREATE INDEX IF NOT EXISTS idx_tag_mismatches_track ON tag_mismatches(track_id);

            -- Performance indexes for tag queries (case-insensitive grouping)
            CREATE INDEX IF NOT EXISTS idx_track_tags_name_value_lower ON track_tags(tag_name, LOWER(tag_value));

            -- Application metadata (version tracking, etc.)
            CREATE TABLE IF NOT EXISTS app_metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            -- Library scan state: stores library file scan results between phases
            -- Written by ScanLibraryDirectory (Awakening), read by DeriveDeployHealthSignals (Awake)
            CREATE TABLE IF NOT EXISTS library_scan_state (
                id INTEGER PRIMARY KEY,
                library_name TEXT NOT NULL,
                library_root TEXT NOT NULL,
                file_path TEXT NOT NULL,
                inode INTEGER NOT NULL,
                scanned_at INTEGER NOT NULL,
                UNIQUE(library_name, file_path)
            );
            CREATE INDEX IF NOT EXISTS idx_library_scan_library ON library_scan_state(library_name);
            CREATE INDEX IF NOT EXISTS idx_library_scan_root ON library_scan_state(library_root);
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
    // Track Queries
    // =========================================================================

    /// Get all tracks, optionally filtered by source.
    pub fn get_all_tracks(&self, source: Option<&str>) -> Result<Vec<super::types::Track>> {
        self.db.get_all_tracks(source)
    }

    /// Get a track by its ID.
    pub fn get_track_by_id(&self, track_id: i64) -> Result<Option<super::types::Track>> {
        self.db.get_track_by_id(track_id)
    }

    /// Get a track by its path.
    pub fn get_track_by_path(&self, path: &str) -> Result<Option<super::types::Track>> {
        self.db.get_track_by_path(path)
    }

    /// Get tracks in a directory for tag editing.
    pub fn get_tracks_for_tag_editing(&self, dir_path: &std::path::Path) -> Result<Vec<super::types::Track>> {
        self.db.get_tracks_for_tag_editing(dir_path)
    }

    /// Get tracks by file types.
    pub fn get_tracks_by_file_types(&self, file_types: &[&str]) -> Result<Vec<(i64, String, String)>> {
        self.db.get_tracks_by_file_types(file_types)
    }

    /// Get tags for a track.
    pub fn get_track_tags(&self, track_id: i64) -> Result<Vec<super::types::TrackTag>> {
        self.db.get_track_tags(track_id)
    }

    /// Get all tracks with their tags (for search functionality).
    pub fn get_all_tracks_with_tags(&self) -> Result<Vec<(super::types::Track, std::collections::HashMap<String, String>)>> {
        self.db.get_all_tracks_with_tags()
    }

    /// Get track counts grouped by file type.
    pub fn get_track_counts_by_file_type(&self) -> Result<std::collections::HashMap<String, i64>> {
        self.db.get_track_counts_by_file_type()
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

    // =========================================================================
    // Library Scan Queries
    // =========================================================================

    /// Get all library scan entries.
    pub fn get_library_scan_files_all(&self) -> Result<Vec<library_scan::LibraryScanEntry>> {
        self.db.get_library_scan_files_all()
    }
}
