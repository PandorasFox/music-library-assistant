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
        // The legacy `files`-targeting migrations (zone-prefix strip, inbox-zone
        // removal, album_art_info reseed) ran in production months ago and are
        // already recorded in `app_metadata`. They were removed during the
        // inode-primary refactor (2026-04) because the `files` table no longer
        // exists in fresh installs.
        DataMigrationEntry {
            id: "2026-03-clear-release-cache-for-tracklists",
            description: "Clear MB release cache to re-fetch with tracklist data (inc=recordings+media)",
            apply: |db| {
                db.conn().execute("DELETE FROM mb_release_cache", [])?;
                Ok(())
            },
        },
        DataMigrationEntry {
            id: "2026-04-wipe-mb-cache-missing-release-group",
            description: "Drop mb_release_cache entries that pre-date the release-group inc parameter (so the CAA release-group fallback can find sibling-release art on refetch)",
            apply: |db| {
                let dropped = db.conn().execute(
                    "DELETE FROM mb_release_cache \
                     WHERE json_extract(CAST(raw_json AS TEXT), '$.\"release-group\".id') IS NULL",
                    [],
                )?;
                crate::logging::log_general(format!(
                    "[MIGRATION] Dropped {} stale mb_release_cache rows lacking release-group",
                    dropped
                ));
                Ok(())
            },
        },
        DataMigrationEntry {
            id: "2026-04-drop-edit-history-tables",
            description: "Drop tag_edit_history and edit_sessions tables (feature removed)",
            apply: |db| {
                db.conn().execute_batch(
                    "DROP TABLE IF EXISTS tag_edit_history;
                     DROP TABLE IF EXISTS edit_sessions;",
                )?;
                Ok(())
            },
        },
        DataMigrationEntry {
            id: "2026-04-split-files-into-inodes-and-inode-paths",
            description: "Split files table into inodes (entity state) + inode_paths (1-to-many path mapping)",
            apply: |db| {
                let conn = db.conn();

                // No-op on fresh installs where `files` never existed.
                let files_exists: bool = conn.query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master \
                     WHERE type = 'table' AND name = 'files'",
                    [],
                    |row| row.get(0),
                )?;
                if !files_exists {
                    crate::logging::log_general(
                        "[MIGRATION] Split files table: no legacy `files` table; skipping",
                    );
                    return Ok(());
                }

                // Backfill `inodes`: one row per distinct inode. For hardlinks
                // (same inode in multiple files rows) the data should agree, but
                // we pick the row with the newest scanned_at as the canonical
                // entity state and log when divergence is detected.
                conn.execute(
                    r#"
                    INSERT OR IGNORE INTO inodes (inode, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
                    SELECT inode, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at
                    FROM files f1
                    WHERE scanned_at = (
                        SELECT MAX(scanned_at) FROM files f2 WHERE f2.inode = f1.inode
                    )
                    "#,
                    [],
                )?;

                // Detect divergence (different mtime/size for same inode) and warn.
                let divergent: i64 = conn.query_row(
                    r#"
                    SELECT COUNT(*) FROM (
                        SELECT inode FROM files
                        GROUP BY inode
                        HAVING COUNT(DISTINCT mtime_secs) > 1
                            OR COUNT(DISTINCT file_size) > 1
                    )
                    "#,
                    [],
                    |row| row.get(0),
                ).unwrap_or(0);
                if divergent > 0 {
                    crate::logging::log_general(format!(
                        "[MIGRATION] {} inodes had divergent mtime/size across files rows; \
                         picked newest scanned_at as canonical",
                        divergent
                    ));
                }

                // Backfill `inode_paths`: every (inode, zone, path) row.
                conn.execute(
                    r#"
                    INSERT OR IGNORE INTO inode_paths (inode, zone, path)
                    SELECT inode, zone, path FROM files
                    "#,
                    [],
                )?;

                // Verify row counts before dropping the source table.
                let files_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM files", [], |row| row.get(0),
                )?;
                let paths_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM inode_paths", [], |row| row.get(0),
                )?;
                let inodes_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM inodes", [], |row| row.get(0),
                )?;
                let distinct_inodes: i64 = conn.query_row(
                    "SELECT COUNT(DISTINCT inode) FROM files", [], |row| row.get(0),
                )?;

                anyhow::ensure!(
                    paths_count == files_count,
                    "Path-row count mismatch: files={}, inode_paths={}",
                    files_count, paths_count,
                );
                anyhow::ensure!(
                    inodes_count == distinct_inodes,
                    "Inode count mismatch: distinct(files.inode)={}, inodes={}",
                    distinct_inodes, inodes_count,
                );

                crate::logging::log_general(format!(
                    "[MIGRATION] Split files table: {} rows -> {} inodes + {} paths",
                    files_count, inodes_count, paths_count,
                ));

                conn.execute_batch("DROP TABLE files")?;

                Ok(())
            },
        },
        DataMigrationEntry {
            id: "2026-03-reseed-compound-tag-signals",
            description: "Re-dirty all compound-tag signal inodes (re-evaluate with plural-form check and MB-skip)",
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
                    SELECT inode, 'compound_tag', ?1
                    FROM signal_compound_tag
                    "#,
                    params![now],
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

#[cfg(test)]
mod tests {
    use super::*;
    use mm_utils::t;

    fn split_migration() -> &'static DataMigrationEntry {
        all_data_migrations()
            .into_iter()
            .find(|m| m.id == "2026-04-split-files-into-inodes-and-inode-paths")
            .map(Box::new)
            .map(Box::leak)
            .expect("migration registered")
    }

    fn fresh_test_db() -> Database {
        Database::open_in_memory()
    }

    fn create_legacy_files_table(db: &Database) {
        t!(db.conn().execute_batch(
            "CREATE TABLE files (
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
        ));
    }

    fn count(db: &Database, sql: &str) -> i64 {
        t!(db.conn().query_row(sql, [], |row| row.get::<_, i64>(0)))
    }

    #[test]
    fn test_split_migration_no_op_on_fresh_db() {
        // Fresh in-memory DB has the new schema (inodes + inode_paths) and no `files` table.
        let db = fresh_test_db();
        let migration = split_migration();
        // Should succeed and do nothing — `files` doesn't exist.
        t!((migration.apply)(&db));
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inodes"), 0);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inode_paths"), 0);
    }

    #[test]
    fn test_split_migration_backfills_simple_files() {
        let db = fresh_test_db();
        create_legacy_files_table(&db);

        // Three distinct inodes, single path each.
        t!(db.conn().execute_batch(
            "INSERT INTO files VALUES (100, 'corpus', 'a.flac', 0, 1, 0, 1024, 1000);
             INSERT INTO files VALUES (101, 'corpus', 'b.flac', 0, 2, 0, 2048, 1001);
             INSERT INTO files VALUES (102, 'library', 'lib/c.flac', 0, 3, 0, 3072, 1002);",
        ));

        let migration = split_migration();
        t!((migration.apply)(&db));

        assert_eq!(count(&db, "SELECT COUNT(*) FROM inodes"), 3);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inode_paths"), 3);

        // Verify a sample row shape.
        let (mtime_secs, file_size): (i64, i64) = t!(db.conn().query_row(
            "SELECT mtime_secs, file_size FROM inodes WHERE inode = 101",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ));
        assert_eq!((mtime_secs, file_size), (2, 2048));

        let path: String = t!(db.conn().query_row(
            "SELECT path FROM inode_paths WHERE inode = 102 AND zone = 'library'",
            [],
            |row| row.get(0),
        ));
        assert_eq!(path, "lib/c.flac");

        // Legacy table dropped.
        let table_exists: i64 = t!(db.conn().query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='files'",
            [],
            |row| row.get(0),
        ));
        assert_eq!(table_exists, 0);
    }

    #[test]
    fn test_split_migration_dedupes_hardlinks_across_zones() {
        let db = fresh_test_db();
        create_legacy_files_table(&db);

        // Same inode 200 at corpus + library paths (cross-zone hardlink).
        t!(db.conn().execute_batch(
            "INSERT INTO files VALUES
                (200, 'corpus',  'corpus/track.flac',  0, 5, 0, 4096, 1100),
                (200, 'library', 'library/track.flac', 0, 5, 0, 4096, 1100);",
        ));

        let migration = split_migration();
        t!((migration.apply)(&db));

        // One inode entity, two paths.
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inodes"), 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inode_paths"), 2);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM inode_paths WHERE inode = 200"),
            2,
        );
    }

    #[test]
    fn test_inode_paths_fk_cascade_on_inode_delete() {
        // Verify the ON DELETE CASCADE FK on inode_paths(inode) → inodes(inode):
        // dropping the inodes row should remove all matching inode_paths rows.
        let db = fresh_test_db();

        t!(db.conn().execute(
            "INSERT INTO inodes (inode, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at) \
             VALUES (500, 0, 1, 0, 100, 1000)",
            [],
        ));
        t!(db.conn().execute_batch(
            "INSERT INTO inode_paths VALUES (500, 'corpus', 'a.flac');
             INSERT INTO inode_paths VALUES (500, 'library', 'lib/a.flac');",
        ));

        assert_eq!(count(&db, "SELECT COUNT(*) FROM inode_paths WHERE inode = 500"), 2);

        t!(db.conn().execute("DELETE FROM inodes WHERE inode = 500", []));

        assert_eq!(count(&db, "SELECT COUNT(*) FROM inode_paths WHERE inode = 500"), 0);
    }

    #[test]
    fn test_inode_paths_zone_independent_deletes() {
        // Deleting the corpus path should not affect a library path with the same inode.
        let db = fresh_test_db();
        t!(db.conn().execute(
            "INSERT INTO inodes (inode, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at) \
             VALUES (600, 0, 1, 0, 100, 1000)",
            [],
        ));
        t!(db.conn().execute_batch(
            "INSERT INTO inode_paths VALUES (600, 'corpus', 'corpus_path.flac');
             INSERT INTO inode_paths VALUES (600, 'library', 'library_path.flac');",
        ));
        t!(db.conn().execute(
            "DELETE FROM inode_paths WHERE inode = 600 AND zone = 'corpus'",
            [],
        ));
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inode_paths WHERE inode = 600"), 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inodes WHERE inode = 600"), 1);
        let remaining_zone: String = t!(db.conn().query_row(
            "SELECT zone FROM inode_paths WHERE inode = 600",
            [],
            |row| row.get(0),
        ));
        assert_eq!(remaining_zone, "library");
    }

    #[test]
    fn test_inode_paths_pk_rejects_duplicate_zone_path() {
        // (inode, zone, path) is the PK — second insert of the same triple errors.
        let db = fresh_test_db();
        t!(db.conn().execute(
            "INSERT INTO inodes (inode, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at) \
             VALUES (700, 0, 1, 0, 100, 1000)",
            [],
        ));
        t!(db.conn().execute(
            "INSERT INTO inode_paths VALUES (700, 'corpus', 'p.flac')",
            [],
        ));
        // Duplicate insert — must fail.
        let dup = db.conn().execute(
            "INSERT INTO inode_paths VALUES (700, 'corpus', 'p.flac')",
            [],
        );
        assert!(dup.is_err(), "duplicate PK insert should fail");

        // INSERT OR IGNORE skips the duplicate (used by the watcher upsert path).
        t!(db.conn().execute(
            "INSERT OR IGNORE INTO inode_paths VALUES (700, 'corpus', 'p.flac')",
            [],
        ));
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inode_paths WHERE inode = 700"), 1);
    }

    #[test]
    fn test_split_migration_picks_newest_scanned_at_on_divergence() {
        let db = fresh_test_db();
        create_legacy_files_table(&db);

        // Same inode but with divergent mtime/size in two rows. Expected: pick
        // the row with the larger scanned_at as the canonical entity.
        t!(db.conn().execute_batch(
            "INSERT INTO files VALUES
                (300, 'corpus',  'old.flac', 0, 10, 0, 100, 500),
                (300, 'library', 'new.flac', 0, 20, 0, 200, 999);",
        ));

        let migration = split_migration();
        t!((migration.apply)(&db));

        let (mtime_secs, file_size, scanned_at): (i64, i64, i64) = t!(db.conn().query_row(
            "SELECT mtime_secs, file_size, scanned_at FROM inodes WHERE inode = 300",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ));
        assert_eq!((mtime_secs, file_size, scanned_at), (20, 200, 999));
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inode_paths"), 2);
    }
}
