//! Database schema definition and version management.
//!
//! Contains the initial schema DDL for new databases and
//! the schema version tracking used by the migration system.

use anyhow::{Context, Result};
use rusqlite::params;

use super::queries::Database;

impl Database {
    /// Create the full current schema from scratch (new databases only).
    ///
    /// This must stay in sync with the cumulative result of all structural
    /// migrations in `MigrationRegistry` (column adds, renames, table
    /// creates/drops, index changes). Data-only migrations like dirty-inode
    /// re-seeding don't apply here.
    pub(in crate::db) fn initialize_schema(&self) -> Result<()> {
        self.conn().execute_batch(
            r#"
            -- =================================================================
            -- Files Table (all paths - files AND directories)
            -- =================================================================
            -- Unified table for all path manifestations across corpus, inbox, and library.
            -- Multiple rows can share the same inode (hard links across corpus + library).
            -- Directories are tracked for scan optimization (skip unchanged dirs).
            CREATE TABLE IF NOT EXISTS files (
                inode INTEGER NOT NULL,
                zone TEXT NOT NULL,             -- 'inbox', 'corpus', 'library'
                path TEXT NOT NULL,             -- relative path (library paths include library name prefix)
                is_dir INTEGER NOT NULL,        -- 1 = directory, 0 = file
                mtime_secs INTEGER NOT NULL,    -- filesystem mtime (same across hard links)
                mtime_nanos INTEGER NOT NULL,
                file_size INTEGER NOT NULL,     -- (same across hard links)
                scanned_at INTEGER NOT NULL,
                PRIMARY KEY (inode, zone, path)
            );

            CREATE INDEX IF NOT EXISTS idx_files_zone ON files(zone);
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
                has_pictures INTEGER NOT NULL DEFAULT 0,
                needs_tag_flush INTEGER NOT NULL DEFAULT 0,
                tags_version INTEGER NOT NULL DEFAULT 0  -- monotonic counter for tag changes
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
            -- Dirty Inodes (incremental computation tracking)
            -- =================================================================
            -- Tracks inodes that need recomputation for specific computation types.
            -- When tags change, inodes are marked dirty here. Computations query
            -- only dirty inodes instead of rescanning the entire corpus.
            CREATE TABLE IF NOT EXISTS dirty_inodes (
                inode INTEGER NOT NULL,
                computation_type TEXT NOT NULL,     -- 'compound_tag', etc.
                dirtied_at INTEGER NOT NULL,        -- unix timestamp
                PRIMARY KEY (inode, computation_type)
            );

            CREATE INDEX IF NOT EXISTS idx_dirty_inodes_type ON dirty_inodes(computation_type);

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

        // Create per-signal typed tables (one table per signal type)
        crate::meta::signals::store::create_all_signal_tables(self.conn())
            .context("Failed to create signal tables")?;

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
            .conn()
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
        self.conn().execute(
            "INSERT OR REPLACE INTO app_metadata (key, value, updated_at) VALUES ('schema_version', ?1, datetime('now'))",
            params![version.to_string()],
        ).context("Failed to set schema version")?;
        Ok(())
    }
}
