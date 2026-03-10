//! Schema inventory — the single source of truth for all database tables.
//!
//! Every table in MM's SQLite database is registered here. The reconciler
//! uses this inventory to detect and apply schema changes at startup.
//!
//! ## Adding a new table
//!
//! 1. Add a `TableEntry` to `schema_inventory()` with the appropriate `TableKind`
//! 2. Done. The reconciler handles creation automatically.
//!
//! ## Changing a table's schema
//!
//! - **Core/Decision tables**: Update the `create_sql` string. The reconciler
//!   detects missing columns and adds them via `ALTER TABLE ADD COLUMN`.
//! - **Computed tables**: Update the `TABLE_SQL` on the signal type. The reconciler
//!   drops and recreates the table, then re-seeds dirty inodes.

use std::hash::{Hash, Hasher};


/// Classification of a table for reconciliation purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    /// Irreplaceable data (files, audio_info, tags, etc.).
    /// Only ADD COLUMN operations are safe.
    Core,
    /// Operator decisions (expected_overlap, canonical_tag, etc.).
    /// Only ADD COLUMN operations are safe.
    Decision,
    /// Recomputable signal data. DROP+CREATE is safe; dirty inodes
    /// are re-seeded afterward to trigger recomputation.
    Computed,
}

/// A single table in the schema inventory.
pub struct TableEntry {
    /// Table name (must match the CREATE TABLE name).
    pub name: &'static str,
    /// Classification for reconciliation safety.
    pub kind: TableKind,
    /// The CREATE TABLE statement (the source of truth).
    pub create_sql: &'static str,
    /// Index creation statements for this table.
    pub index_sql: &'static [&'static str],
}

/// Returns the complete schema inventory — every table in MM's database.
///
/// This is the single source of truth. `initialize_schema()` iterates this
/// to create tables in fresh databases. The reconciler compares this against
/// the actual database to detect drift.
#[allow(clippy::vec_init_then_push)] // 50+ entries, push pattern is clearer with section comments
pub fn schema_inventory() -> Vec<TableEntry> {
    let mut tables = Vec::new();

    // =================================================================
    // Core tables (irreplaceable data)
    // =================================================================

    tables.push(TableEntry {
        name: "files",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS files (
            inode INTEGER NOT NULL,
            zone TEXT NOT NULL,
            path TEXT NOT NULL,
            is_dir INTEGER NOT NULL,
            mtime_secs INTEGER NOT NULL,
            mtime_nanos INTEGER NOT NULL,
            file_size INTEGER NOT NULL,
            scanned_at INTEGER NOT NULL,
            PRIMARY KEY (inode, zone, path)
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_files_zone ON files(zone)",
            "CREATE INDEX IF NOT EXISTS idx_files_inode ON files(inode)",
            "CREATE INDEX IF NOT EXISTS idx_files_is_dir ON files(is_dir)",
            "CREATE INDEX IF NOT EXISTS idx_files_path ON files(path)",
        ],
    });

    tables.push(TableEntry {
        name: "audio_info",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS audio_info (
            inode INTEGER PRIMARY KEY,
            file_type TEXT NOT NULL,
            duration_ms INTEGER,
            bitrate_kbps INTEGER,
            sample_rate INTEGER,
            fingerprint BLOB,
            has_pictures INTEGER NOT NULL DEFAULT 0,
            pic_format TEXT,
            pic_width INTEGER,
            pic_height INTEGER,
            pic_count INTEGER NOT NULL DEFAULT 0,
            needs_tag_flush INTEGER NOT NULL DEFAULT 0,
            tags_version INTEGER NOT NULL DEFAULT 0
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_audio_info_fingerprint ON audio_info(fingerprint)",
            "CREATE INDEX IF NOT EXISTS idx_audio_info_duration ON audio_info(duration_ms)",
        ],
    });

    tables.push(TableEntry {
        name: "corpus_tags",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS corpus_tags (
            inode INTEGER NOT NULL REFERENCES audio_info(inode) ON DELETE CASCADE,
            tag_name TEXT NOT NULL,
            tag_value TEXT NOT NULL,
            PRIMARY KEY (inode, tag_name, tag_value)
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_corpus_tags_inode ON corpus_tags(inode)",
            "CREATE INDEX IF NOT EXISTS idx_corpus_tags_name ON corpus_tags(tag_name)",
            "CREATE INDEX IF NOT EXISTS idx_corpus_tags_name_value ON corpus_tags(tag_name, tag_value)",
            "CREATE INDEX IF NOT EXISTS idx_corpus_tags_name_value_lower ON corpus_tags(tag_name, LOWER(tag_value))",
        ],
    });

    tables.push(TableEntry {
        name: "inbox_tags",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS inbox_tags (
            inode INTEGER NOT NULL REFERENCES audio_info(inode) ON DELETE CASCADE,
            tag_name TEXT NOT NULL,
            tag_value TEXT NOT NULL,
            PRIMARY KEY (inode, tag_name, tag_value)
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_inbox_tags_inode ON inbox_tags(inode)",
            "CREATE INDEX IF NOT EXISTS idx_inbox_tags_name ON inbox_tags(tag_name)",
        ],
    });

    tables.push(TableEntry {
        name: "tag_edit_history",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS tag_edit_history (
            id INTEGER PRIMARY KEY,
            inode INTEGER NOT NULL REFERENCES audio_info(inode) ON DELETE CASCADE,
            field_name TEXT NOT NULL,
            old_value TEXT,
            new_value TEXT,
            edited_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            session_id TEXT
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_tag_history_inode ON tag_edit_history(inode)",
            "CREATE INDEX IF NOT EXISTS idx_tag_history_session ON tag_edit_history(session_id)",
        ],
    });

    tables.push(TableEntry {
        name: "corpus_health_stats",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS corpus_health_stats (
            id INTEGER PRIMARY KEY,
            stat_type TEXT NOT NULL,
            last_updated DATETIME DEFAULT CURRENT_TIMESTAMP,
            data_json TEXT NOT NULL
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_corpus_health_type ON corpus_health_stats(stat_type)",
        ],
    });

    tables.push(TableEntry {
        name: "image_info",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS image_info (
            inode INTEGER PRIMARY KEY,
            format TEXT NOT NULL,
            width INTEGER NOT NULL DEFAULT 0,
            height INTEGER NOT NULL DEFAULT 0,
            role TEXT NOT NULL DEFAULT 'other'
        )",
        index_sql: &["CREATE INDEX IF NOT EXISTS idx_image_info_role ON image_info(role)"],
    });

    tables.push(TableEntry {
        name: "dirty_inodes",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS dirty_inodes (
            inode INTEGER NOT NULL,
            computation_type TEXT NOT NULL,
            dirtied_at INTEGER NOT NULL,
            PRIMARY KEY (inode, computation_type)
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_dirty_inodes_type ON dirty_inodes(computation_type)",
        ],
    });

    tables.push(TableEntry {
        name: "app_metadata",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS app_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        index_sql: &[],
    });

    tables.push(TableEntry {
        name: "external_matches",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS external_matches (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            inode INTEGER NOT NULL,
            fingerprint BLOB NOT NULL,
            source INTEGER NOT NULL,
            recording_id TEXT NOT NULL,
            confidence REAL NOT NULL,
            raw_response BLOB,
            fetched_at INTEGER NOT NULL,
            UNIQUE(inode, source, recording_id)
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_external_matches_inode ON external_matches(inode)",
            "CREATE INDEX IF NOT EXISTS idx_external_matches_fp ON external_matches(fingerprint)",
        ],
    });

    tables.push(TableEntry {
        name: "external_no_match",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS external_no_match (
            fingerprint BLOB NOT NULL,
            source INTEGER NOT NULL,
            queried_at INTEGER NOT NULL,
            PRIMARY KEY (fingerprint, source)
        )",
        index_sql: &[],
    });

    tables.push(TableEntry {
        name: "mb_recording_cache",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS mb_recording_cache (
            recording_id TEXT PRIMARY KEY,
            raw_json BLOB NOT NULL,
            fetched_at INTEGER NOT NULL
        )",
        index_sql: &[],
    });

    tables.push(TableEntry {
        name: "mb_artist_cache",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS mb_artist_cache (
            artist_id TEXT PRIMARY KEY,
            raw_json BLOB NOT NULL,
            fetched_at INTEGER NOT NULL
        )",
        index_sql: &[],
    });

    tables.push(TableEntry {
        name: "mb_release_cache",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS mb_release_cache (
            release_id TEXT PRIMARY KEY,
            raw_json BLOB NOT NULL,
            fetched_at INTEGER NOT NULL
        )",
        index_sql: &[],
    });

    tables.push(TableEntry {
        name: "mb_known_entities",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS mb_known_entities (
            mbid TEXT NOT NULL,
            entity_type TEXT NOT NULL,
            discovered_from TEXT,
            discovered_at INTEGER NOT NULL,
            PRIMARY KEY (mbid, entity_type)
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_mb_known_entities_type ON mb_known_entities(entity_type)",
        ],
    });

    tables.push(TableEntry {
        name: "external_retry",
        kind: TableKind::Core,
        create_sql: "CREATE TABLE IF NOT EXISTS external_retry (
            inode INTEGER NOT NULL,
            fingerprint BLOB NOT NULL,
            source INTEGER NOT NULL,
            failed_at INTEGER NOT NULL,
            error TEXT,
            retry_count INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (inode, source)
        )",
        index_sql: &[],
    });

    // =================================================================
    // Signal tables (Decision + Computed) — generated by signal_registry!
    // =================================================================

    tables.extend(crate::meta::signals::registry::signal_table_entries());

    // =================================================================
    // Intermediate computed tables (pipeline scratch data)
    // =================================================================

    tables.push(TableEntry {
        name: "release_packing_manifest",
        kind: TableKind::Computed,
        create_sql: "CREATE TABLE IF NOT EXISTS release_packing_manifest (
            release_id TEXT PRIMARY KEY,
            total_tracks INTEGER NOT NULL,
            release_title TEXT NOT NULL,
            release_artist TEXT NOT NULL,
            media_count INTEGER NOT NULL DEFAULT 1
        )",
        index_sql: &[],
    });

    tables.push(TableEntry {
        name: "release_packing_candidates",
        kind: TableKind::Computed,
        create_sql: "CREATE TABLE IF NOT EXISTS release_packing_candidates (
            release_id TEXT NOT NULL,
            inode INTEGER NOT NULL,
            recording_id TEXT NOT NULL,
            confidence REAL NOT NULL,
            path TEXT NOT NULL,
            parent_dir TEXT NOT NULL,
            duration_ms INTEGER,
            tag_title TEXT,
            tag_artist TEXT,
            tag_album TEXT,
            tag_tracknumber TEXT,
            dir_file_count INTEGER NOT NULL DEFAULT 1,
            PRIMARY KEY (release_id, inode)
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_rpc_release ON release_packing_candidates(release_id)",
        ],
    });

    tables.push(TableEntry {
        name: "release_packing_scores",
        kind: TableKind::Computed,
        create_sql: "CREATE TABLE IF NOT EXISTS release_packing_scores (
            release_id TEXT NOT NULL,
            inode INTEGER NOT NULL,
            recording_id TEXT NOT NULL,
            medium_pos INTEGER NOT NULL,
            track_pos INTEGER NOT NULL,
            track_title TEXT NOT NULL,
            medium_format TEXT,
            track_number TEXT NOT NULL,
            score REAL NOT NULL,
            score_breakdown BLOB NOT NULL,
            is_optimal INTEGER NOT NULL DEFAULT 0,
            match_method INTEGER NOT NULL DEFAULT 0,
            fingerprint_hex TEXT,
            raw_duration_ms INTEGER,
            PRIMARY KEY (release_id, inode, medium_pos, track_pos)
        )",
        index_sql: &[
            "CREATE INDEX IF NOT EXISTS idx_rps_optimal ON release_packing_scores(is_optimal)",
            "CREATE INDEX IF NOT EXISTS idx_rps_inode ON release_packing_scores(inode)",
        ],
    });

    tables.push(TableEntry {
        name: "pending_acoustid_submissions",
        kind: TableKind::Computed,
        create_sql: "CREATE TABLE IF NOT EXISTS pending_acoustid_submissions (
            fingerprint TEXT NOT NULL,
            recording_id TEXT NOT NULL,
            duration_ms INTEGER NOT NULL,
            source TEXT NOT NULL DEFAULT 'elimination',
            PRIMARY KEY (fingerprint, recording_id)
        )",
        index_sql: &[],
    });

    tables
}

/// Compute a deterministic fingerprint of the entire schema inventory.
///
/// Hash of all CREATE TABLE + index SQL strings. A match at startup means
/// the schema is up-to-date and reconciliation can be skipped (fast path).
pub fn schema_fingerprint() -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    for entry in schema_inventory() {
        entry.name.hash(&mut hasher);
        entry.create_sql.hash(&mut hasher);
        for idx in entry.index_sql {
            idx.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Key used to store the schema fingerprint in `app_metadata`.
pub const FINGERPRINT_KEY: &str = "schema_fingerprint";
