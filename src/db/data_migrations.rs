//! Manual data-only migration registry.
//!
//! For data transformations the reconciler can't auto-detect: re-seeding
//! dirty inodes, normalizing values, backfilling computed columns, etc.
//!
//! Each applied migration records `data_migration:{id}` in `app_metadata`.
//! The reconciler checks which IDs are already applied and skips them.
//!
//! ## Adding a data migration
//!
//! 1. Add a `DataMigrationEntry` to `all_data_migrations()` with a unique ID
//! 2. Done. The reconciler runs it once and records the ID.

use anyhow::Result;

use super::queries::Database;

/// A single data-only migration.
pub struct DataMigrationEntry {
    /// Unique identifier (e.g. "2026-03-reseed-compound-tags").
    /// Stored in `app_metadata` as `data_migration:{id}` after application.
    pub id: &'static str,
    /// Human-readable description for the approval UI.
    pub description: &'static str,
    /// The migration function to execute.
    pub apply: fn(&Database) -> Result<()>,
}

/// Returns all registered data migrations.
pub fn all_data_migrations() -> Vec<DataMigrationEntry> {
    vec![
        DataMigrationEntry {
            id: "2026-02-reseed-album-art-info",
            description: "Re-seed album_art_info dirty inodes (fix: backfill was using unresolved relative paths)",
            apply: |db| {
                use rusqlite::params;
                use std::time::{SystemTime, UNIX_EPOCH};
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                db.conn().execute(
                    r#"
                    INSERT OR IGNORE INTO dirty_inodes (inode, computation_type, dirtied_at)
                    SELECT a.inode, 'album_art_info', ?1
                    FROM audio_info a
                    JOIN files f ON a.inode = f.inode
                    WHERE f.zone = 'corpus' AND a.has_pictures = 1
                    "#,
                    params![now],
                )?;
                Ok(())
            },
        },
        DataMigrationEntry {
            id: "2026-03-clear-release-cache-for-tracklists",
            description: "Clear MB release cache to re-fetch with tracklist data (inc=recordings+media)",
            apply: |db| {
                db.conn().execute("DELETE FROM mb_release_cache", [])?;
                Ok(())
            },
        },
        DataMigrationEntry {
            id: "2026-03-strip-zone-prefix-from-paths",
            description: "Strip zone prefix from file paths (zone is metadata, not path)",
            apply: |db| {
                let conn = db.conn();
                // Corpus: strip "corpus/" prefix (7 chars, SUBSTR 1-based → start at 8)
                conn.execute(
                    "UPDATE files SET path = SUBSTR(path, 8) \
                     WHERE zone = 'corpus' AND path LIKE 'corpus/%'",
                    [],
                )?;
                // Inbox: strip "inbox/" prefix (6 chars → start at 7)
                conn.execute(
                    "UPDATE files SET path = SUBSTR(path, 7) \
                     WHERE zone = 'inbox' AND path LIKE 'inbox/%'",
                    [],
                )?;
                // Library: delete buggy "library/"-prefixed entries (sidecar images).
                // Correct entries already exist from ReconcileLibraryFiles.
                conn.execute(
                    "DELETE FROM files \
                     WHERE zone = 'library' AND path LIKE 'library/%'",
                    [],
                )?;
                Ok(())
            },
        },
        DataMigrationEntry {
            id: "2026-03-backfill-edit-sessions",
            description: "Backfill edit_sessions summary table from tag_edit_history",
            apply: |db| {
                db.conn().execute(
                    "INSERT OR IGNORE INTO edit_sessions (session_id, created_at, edit_count, inode_count)
                     SELECT session_id, MIN(edited_at), COUNT(*), COUNT(DISTINCT inode)
                     FROM tag_edit_history
                     GROUP BY session_id",
                    [],
                )?;
                Ok(())
            },
        },
        DataMigrationEntry {
            id: "2026-03-remove-inbox-zone",
            description: "Remove inbox zone: delete inbox files and drop orphan inbox signal/tag tables",
            apply: |db| {
                let conn = db.conn();
                conn.execute("DELETE FROM files WHERE zone = 'inbox'", [])?;
                conn.execute_batch(
                    "DROP TABLE IF EXISTS inbox_tags;
                     DROP TABLE IF EXISTS signal_file_in_inbox;
                     DROP TABLE IF EXISTS signal_inbox_unindexed;
                     DROP TABLE IF EXISTS signal_inbox_healthy;
                     DROP TABLE IF EXISTS signal_inbox_corpus_match;
                     DROP TABLE IF EXISTS signal_inbox_compound_tag;
                     DROP TABLE IF EXISTS signal_inbox_tag_canonicity;
                     DROP TABLE IF EXISTS signal_inbox_missing_tag;",
                )?;
                Ok(())
            },
        },
    ]
}

/// Check whether a data migration has already been applied.
pub fn is_migration_applied(db: &Database, id: &str) -> bool {
    let key = format!("data_migration:{}", id);
    db.conn()
        .query_row(
            "SELECT COUNT(*) > 0 FROM app_metadata WHERE key = ?1",
            [&key],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
}

/// Mark a data migration as applied.
pub fn mark_migration_applied(db: &Database, id: &str) -> Result<()> {
    let key = format!("data_migration:{}", id);
    db.conn().execute(
        "INSERT OR REPLACE INTO app_metadata (key, value, updated_at) VALUES (?1, 'applied', datetime('now'))",
        [&key],
    )?;
    Ok(())
}
