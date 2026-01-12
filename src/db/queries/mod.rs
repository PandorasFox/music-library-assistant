//! Database operations and queries.
//!
//! All SQLite operations are organized into domain-specific submodules:
//! - `tracks`: Track CRUD and queries by source/path/fingerprint/metadata
//! - `scan_state`: Incremental scan state tracking
//! - `deployment`: Deployment logging and library track queries
//! - `duplicates`: Duplicate group management and resolution
//! - `changes`: Change sessions and pending changes
//! - `health`: Health issues, summaries, known variants
//! - `metadata`: App metadata, artist canonicalization, tag mismatches

#![allow(dead_code)]

mod changes;
mod deployment;
mod duplicates;
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
        db.migrate_add_fingerprint()?;
        db.migrate_add_isrc()?;
        Ok(db)
    }

    fn initialize_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tracks (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                source TEXT NOT NULL,
                inode INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                file_type TEXT NOT NULL,
                artist TEXT,
                album TEXT,
                album_artist TEXT,
                title TEXT,
                track_number INTEGER,
                duration_ms INTEGER,
                bitrate_kbps INTEGER,
                sample_rate INTEGER,
                fingerprint TEXT,
                scanned_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_source ON tracks(source);
            CREATE INDEX IF NOT EXISTS idx_inode ON tracks(inode);
            CREATE INDEX IF NOT EXISTS idx_artist ON tracks(artist);
            CREATE INDEX IF NOT EXISTS idx_album ON tracks(album);
            CREATE INDEX IF NOT EXISTS idx_album_artist ON tracks(album_artist);
            CREATE INDEX IF NOT EXISTS idx_title ON tracks(title);
            CREATE INDEX IF NOT EXISTS idx_duration ON tracks(duration_ms);
            CREATE INDEX IF NOT EXISTS idx_fingerprint ON tracks(fingerprint);

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

            CREATE TABLE IF NOT EXISTS duplicate_groups (
                id INTEGER PRIMARY KEY,
                group_type TEXT NOT NULL,
                group_key TEXT NOT NULL,
                resolution_state TEXT DEFAULT 'pending',
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                resolved_at DATETIME
            );
            CREATE INDEX IF NOT EXISTS idx_duplicate_group_type ON duplicate_groups(group_type);
            CREATE INDEX IF NOT EXISTS idx_duplicate_resolution ON duplicate_groups(resolution_state);

            CREATE TABLE IF NOT EXISTS duplicate_group_members (
                id INTEGER PRIMARY KEY,
                group_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                selected_for_keep BOOLEAN DEFAULT 0,
                FOREIGN KEY(group_id) REFERENCES duplicate_groups(id),
                FOREIGN KEY(track_id) REFERENCES tracks(id)
            );
            CREATE INDEX IF NOT EXISTS idx_duplicate_members_group ON duplicate_group_members(group_id);
            CREATE INDEX IF NOT EXISTS idx_duplicate_members_track ON duplicate_group_members(track_id);

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

            -- Algebraic change tracking tables
            CREATE TABLE IF NOT EXISTS pending_changes (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                change_type TEXT NOT NULL,
                source_path TEXT NOT NULL,
                target_path TEXT,
                metadata_changes TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                status TEXT DEFAULT 'pending'
            );
            CREATE INDEX IF NOT EXISTS idx_pending_changes_session ON pending_changes(session_id);
            CREATE INDEX IF NOT EXISTS idx_pending_changes_status ON pending_changes(status);
            CREATE INDEX IF NOT EXISTS idx_pending_changes_source ON pending_changes(source_path);

            CREATE TABLE IF NOT EXISTS change_sessions (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL UNIQUE,
                description TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                committed_at DATETIME,
                status TEXT DEFAULT 'active'
            );
            CREATE INDEX IF NOT EXISTS idx_change_sessions_status ON change_sessions(status);

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

            -- Canonical artist mappings
            CREATE TABLE IF NOT EXISTS artist_canonicalization (
                id INTEGER PRIMARY KEY,
                canonical_name TEXT NOT NULL,
                variant_name TEXT NOT NULL UNIQUE,
                confidence REAL,
                auto_detected INTEGER DEFAULT 1,
                confirmed_at DATETIME
            );
            CREATE INDEX IF NOT EXISTS idx_artist_canon_canonical ON artist_canonicalization(canonical_name);
            CREATE INDEX IF NOT EXISTS idx_artist_canon_variant ON artist_canonicalization(variant_name);

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

    /// Migrate existing databases to add fingerprint column
    fn migrate_add_fingerprint(&self) -> Result<()> {
        let has_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tracks') WHERE name='fingerprint'",
                params![],
                |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count > 0)
                },
            )
            .unwrap_or(false);

        if !has_column {
            self.conn
                .execute("ALTER TABLE tracks ADD COLUMN fingerprint TEXT", params![])
                .context("Failed to add fingerprint column")?;

            self.conn
                .execute(
                    "CREATE INDEX IF NOT EXISTS idx_fingerprint ON tracks(fingerprint)",
                    params![],
                )
                .context("Failed to create fingerprint index")?;
        }

        Ok(())
    }

    /// Migrate existing databases to add ISRC column
    fn migrate_add_isrc(&self) -> Result<()> {
        let has_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tracks') WHERE name='isrc'",
                params![],
                |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count > 0)
                },
            )
            .unwrap_or(false);

        if !has_column {
            self.conn
                .execute("ALTER TABLE tracks ADD COLUMN isrc TEXT", params![])
                .context("Failed to add ISRC column")?;

            self.conn
                .execute(
                    "CREATE INDEX IF NOT EXISTS idx_isrc ON tracks(isrc)",
                    params![],
                )
                .context("Failed to create ISRC index")?;
        }

        Ok(())
    }

    /// Run VACUUM to compact the database after bulk deletions.
    pub fn vacuum(&self) -> Result<()> {
        self.conn
            .execute("VACUUM", [])
            .context("Failed to vacuum database")?;
        Ok(())
    }
}
