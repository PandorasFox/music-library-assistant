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
mod metadata;
mod scan_state;
mod tracks;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;

// Types are re-exported from db/mod.rs, not here.
// This module only exposes the Database struct.

/// Central database connection wrapper.
pub struct Database {
    pub(crate) conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open database")?;

        // Enable foreign key constraint enforcement
        conn.execute("PRAGMA foreign_keys = ON", [])
            .context("Failed to enable foreign key constraints")?;

        let db = Database { conn };
        db.initialize_schema()?;
        Ok(db)
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
            -- Corpus Health Tracking Tables
            -- =================================================================

            -- Health issues discovered in corpus
            CREATE TABLE IF NOT EXISTS health_issues (
                id INTEGER PRIMARY KEY,
                issue_type TEXT NOT NULL,
                issue_key TEXT NOT NULL,
                severity TEXT NOT NULL,
                discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                resolved_at DATETIME,
                resolution_type TEXT,
                resolution_session TEXT,
                metadata_json TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_health_issues_type ON health_issues(issue_type);
            CREATE INDEX IF NOT EXISTS idx_health_issues_key ON health_issues(issue_key);
            CREATE INDEX IF NOT EXISTS idx_health_issues_severity ON health_issues(severity);
            CREATE INDEX IF NOT EXISTS idx_health_issues_unresolved
                ON health_issues(issue_type) WHERE resolved_at IS NULL;

            -- Tracks involved in each health issue
            CREATE TABLE IF NOT EXISTS health_issue_tracks (
                id INTEGER PRIMARY KEY,
                issue_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                FOREIGN KEY(issue_id) REFERENCES health_issues(id) ON DELETE CASCADE,
                FOREIGN KEY(track_id) REFERENCES tracks(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_health_issue_tracks_issue ON health_issue_tracks(issue_id);
            CREATE INDEX IF NOT EXISTS idx_health_issue_tracks_track ON health_issue_tracks(track_id);

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

            -- Unified tag canonicalization (artist, album_artist, genre, album)
            CREATE TABLE IF NOT EXISTS tag_canonicalization (
                id INTEGER PRIMARY KEY,
                tag_name TEXT NOT NULL,
                canonical_value TEXT NOT NULL,
                variant_value TEXT NOT NULL,
                confidence REAL,
                auto_detected INTEGER DEFAULT 1,
                confirmed_at DATETIME,
                UNIQUE(tag_name, variant_value)
            );
            CREATE INDEX IF NOT EXISTS idx_tag_canon_name ON tag_canonicalization(tag_name);
            CREATE INDEX IF NOT EXISTS idx_tag_canon_canonical ON tag_canonicalization(tag_name, canonical_value);
            CREATE INDEX IF NOT EXISTS idx_tag_canon_unconfirmed ON tag_canonicalization(tag_name) WHERE confirmed_at IS NULL;

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
            "#
        ).context("Failed to initialize database schema")?;

        Ok(())
    }

    /// Run VACUUM to compact the database after bulk deletions.
    pub fn vacuum(&self) -> Result<()> {
        self.conn
            .execute("VACUUM", [])
            .context("Failed to vacuum database")?;
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
                // No version recorded - this is a pre-versioning database
                // Set baseline version and return it
                self.set_schema_version(2)?;
                Ok(2)
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
