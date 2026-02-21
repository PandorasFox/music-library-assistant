//! Database schema migration system.
//!
//! Provides versioned, forward-only migrations that run at startup.
//! Each migration transforms the schema from one version to the next.
//!
//! ## Idempotency Requirement
//!
//! **All migrations MUST be idempotent.** A migration may run on a database
//! where the schema changes have already been applied (e.g., via initialize_schema
//! or a previous partial run). Use patterns like:
//! - `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS`
//! - Check column existence via `pragma_table_info` before `ALTER TABLE ADD COLUMN`
//! - Guard other operations with existence checks
//!
//! ## Keeping in Sync with `initialize_schema`
//!
//! `Database::initialize_schema()` creates the full current schema for new
//! databases. Any structural migration here (column add/rename, table
//! create/drop, index change) must be reflected there too, so that a fresh
//! database matches one that has been migrated through all versions.
//! Data-only migrations (dirty-inode re-seeding, tag uppercasing) don't
//! need a counterpart in `initialize_schema`.
//!
//! ## Current State
//!
//! Schema v1 is the inode-based files/audio_info/corpus_tags schema.
//! This is a fresh start - no migrations from previous schemas exist.

use anyhow::{Context, Result};
use rusqlite::params;

use crate::db::Database;
use crate::db::ReadOnlyDb;
use crate::witch::MaintenanceWitness;

/// Re-seed all corpus inodes as dirty for a given computation type.
///
/// Use this in migrations when signal data has been lost or invalidated
/// (e.g., table migrations that don't carry data forward) and the
/// incremental dirty-inode system needs a full recomputation pass.
///
/// Only seeds corpus files (not library files), using `INSERT OR IGNORE`
/// so already-dirty inodes are preserved.
pub fn seed_dirty_inodes_for(db: &Database, computation_type: &str) -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    db.conn().execute(
        r#"
        INSERT OR IGNORE INTO dirty_inodes (inode, computation_type, dirtied_at)
        SELECT a.inode, ?1, ?2
        FROM audio_info a
        JOIN files f ON a.inode = f.inode
        WHERE f.zone = 'corpus'
        "#,
        params![computation_type, now],
    )?;
    Ok(())
}

/// Idempotently add a column to a table if it doesn't already exist.
fn add_column_if_missing(
    conn: &rusqlite::Connection,
    table: &str,
    column: &str,
    column_def: &str,
) -> Result<()> {
    let has_column: bool = conn.query_row(
        &format!(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('{}') WHERE name = '{}'",
            table, column
        ),
        [],
        |row| row.get(0),
    )?;

    if !has_column {
        conn.execute(
            &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, column_def),
            [],
        )?;
    }
    Ok(())
}

/// A single database migration.
pub struct Migration {
    /// Version number this migration starts from.
    pub from_version: u32,
    /// Version number after migration completes.
    pub to_version: u32,
    /// Human-readable description of the migration.
    pub description: &'static str,
    /// The migration function to execute.
    pub apply: fn(&Database) -> Result<()>,
}

/// Registry of all database migrations.
///
/// Migrations are applied in order, starting from the current schema version.
pub struct MigrationRegistry {
    migrations: Vec<Migration>,
}

impl MigrationRegistry {
    /// Create a new migration registry with all known migrations.
    pub fn new() -> Self {
        let mut registry = Self {
            migrations: Vec::new(),
        };

        // v1→v2: Add tags_version column to audio_info and create dirty_inodes table
        registry.register(Migration {
            from_version: 1,
            to_version: 2,
            description: "Add tags_version column and dirty_inodes table for incremental computation",
            apply: |db| {
                // Check if tags_version column already exists (idempotent)
                let has_tags_version: bool = db.conn().query_row(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('audio_info') WHERE name = 'tags_version'",
                    [],
                    |row| row.get(0),
                )?;

                if !has_tags_version {
                    db.conn().execute(
                        "ALTER TABLE audio_info ADD COLUMN tags_version INTEGER NOT NULL DEFAULT 0",
                        [],
                    )?;
                }

                // Create dirty_inodes table (IF NOT EXISTS is already idempotent)
                db.conn().execute_batch(
                    r#"
                    CREATE TABLE IF NOT EXISTS dirty_inodes (
                        inode INTEGER NOT NULL,
                        computation_type TEXT NOT NULL,
                        dirtied_at INTEGER NOT NULL,
                        PRIMARY KEY (inode, computation_type)
                    );
                    CREATE INDEX IF NOT EXISTS idx_dirty_inodes_type ON dirty_inodes(computation_type);
                    "#
                )?;
                Ok(())
            },
        });

        // v2→v3: Add native inode column to signals table for inode-keyed signals
        registry.register(Migration {
            from_version: 2,
            to_version: 3,
            description: "Add inode column to signals table for proper inode-keyed signal storage",
            apply: |db| {
                // Check if inode column already exists (idempotent)
                let has_inode_column: bool = db.conn().query_row(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('signals') WHERE name = 'inode'",
                    [],
                    |row| row.get(0),
                )?;

                if !has_inode_column {
                    db.conn().execute(
                        "ALTER TABLE signals ADD COLUMN inode INTEGER",
                        [],
                    )?;
                }

                // Create index on inode column (IF NOT EXISTS is idempotent)
                db.conn().execute(
                    "CREATE INDEX IF NOT EXISTS idx_signals_inode ON signals(inode)",
                    [],
                )?;

                // Migrate existing inode-keyed signals: parse issue_key as integer and store in inode column
                // These are corpus file signals where issue_key stores inode.to_string()
                let inode_keyed_types = [
                    "file_in_corpus", "unindexed_file", "healthy_file", "missing_file",
                    "moved_file", "oob_tag_sync", "oob_tag_conflict", "mtime_only_mismatch",
                    "corrupt_file", "shit_format", "subpar_duplicate", "compound_tag",
                    "deploy_ready", "deployed_healthy",
                ];

                for signal_type in inode_keyed_types {
                    // Only update rows where issue_key looks like an integer (all digits)
                    // and inode column is still NULL
                    db.conn().execute(
                        &format!(
                            r#"UPDATE signals
                               SET inode = CAST(issue_key AS INTEGER)
                               WHERE issue_type = '{}'
                               AND inode IS NULL
                               AND issue_key GLOB '[0-9]*'
                               AND issue_key NOT GLOB '*[^0-9]*'"#,
                            signal_type
                        ),
                        [],
                    )?;
                }

                // Delete obsolete InodeChanged signals
                // These are now exposed as MissingFile + UnindexedFile pair
                db.conn().execute(
                    "DELETE FROM signals WHERE issue_type = 'inode_changed'",
                    [],
                )?;

                Ok(())
            },
        });

        // v3→v4: Create per-signal typed tables (replacing single signals + metadata_json)
        registry.register(Migration {
            from_version: 3,
            to_version: 4,
            description: "Create per-signal typed tables for all corpus and aggregate signal types",
            apply: |db| {
                crate::meta::signals::store::create_all_signal_tables(db.conn())?;
                Ok(())
            },
        });

        // v4→v5: Drop old signals table (all data now in per-signal typed tables)
        registry.register(Migration {
            from_version: 4,
            to_version: 5,
            description: "Drop legacy signals table — per-signal typed tables are now sole source of truth",
            apply: |db| {
                // Drop the old monolithic signals table and its indices.
                // All signal data is now stored in per-signal typed tables
                // (signal_file_in_corpus, signal_missing_file, etc.)
                db.conn().execute_batch(
                    r#"
                    DROP TABLE IF EXISTS signals;
                    "#
                )?;
                Ok(())
            },
        });

        // v5→v6: Re-seed dirty inodes for compound tag detection
        //
        // The v3→v4 typed table migration created signal_compound_tag fresh but
        // did not migrate data from the old signals table. v4→v5 then dropped
        // the old table, losing all compound tag signals. Since dirty inode flags
        // were already consumed, detection never re-runs. This re-seeds all corpus
        // inodes so compound tag detection repopulates the typed table.
        registry.register(Migration {
            from_version: 5,
            to_version: 6,
            description: "Re-seed dirty inodes for compound tag detection after typed table migration data loss",
            apply: |db| {
                seed_dirty_inodes_for(db, "compound_tag")
            },
        });

        // v6→v7: Re-seed dirty inodes for compound tag detection after split rule reorder
        //
        // ba9b502 changed compound tag splitting to prioritize semicolons over " & "
        // (e.g., "Aly & Fila; JES" → ["Aly & Fila", "JES"] instead of ["Aly", "Fila; JES"]).
        // Existing signals were computed under the old rule order and are stale.
        // Re-seeding forces recomputation under the new priority chain.
        registry.register(Migration {
            from_version: 6,
            to_version: 7,
            description: "Re-seed compound tag detection after split rule priority reorder",
            apply: |db| {
                seed_dirty_inodes_for(db, "compound_tag")
            },
        });

        // v7→v8: Uppercase all tag names to match TagSet's new UPPERCASE normalization
        //
        // TagSet now normalizes keys to UPPERCASE (matching VorbisComments convention
        // on disk). All tag_name columns in the DB need to be uppercased to match.
        // Signal tables that embed tag names in keys or bincode blobs are cleared
        // and will be rebuilt by the next computation cycle.
        registry.register(Migration {
            from_version: 7,
            to_version: 8,
            description: "Uppercase all tag names to match TagSet UPPERCASE normalization",
            apply: |db| {
                let conn = db.conn();

                // corpus_tags: PK is (inode, tag_name, tag_value).
                // Insert uppercased versions (IGNORE if already exists), then delete old lowercase rows.
                // Idempotent: WHERE clause only matches rows not already uppercase.
                conn.execute(
                    r#"INSERT OR IGNORE INTO corpus_tags (inode, tag_name, tag_value)
                       SELECT inode, UPPER(tag_name), tag_value
                       FROM corpus_tags WHERE tag_name != UPPER(tag_name)"#,
                    [],
                )?;
                conn.execute(
                    "DELETE FROM corpus_tags WHERE tag_name != UPPER(tag_name)",
                    [],
                )?;

                // inbox_tags: same PK structure, same pattern.
                conn.execute(
                    r#"INSERT OR IGNORE INTO inbox_tags (inode, tag_name, tag_value)
                       SELECT inode, UPPER(tag_name), tag_value
                       FROM inbox_tags WHERE tag_name != UPPER(tag_name)"#,
                    [],
                )?;
                conn.execute(
                    "DELETE FROM inbox_tags WHERE tag_name != UPPER(tag_name)",
                    [],
                )?;

                // tag_edit_history: no unique constraint on field_name, simple UPDATE.
                conn.execute(
                    "UPDATE tag_edit_history SET field_name = UPPER(field_name) WHERE field_name != UPPER(field_name)",
                    [],
                )?;

                // Signal tables with embedded tag names: clear and let recomputation rebuild.
                // signal_canonical_tag: key = "{tag_name}:{value}", tag_name column
                // Created by mark_canonical_tag mutations — these are operator decisions
                // that will need to be re-established. Updating in-place:
                conn.execute(
                    r#"UPDATE signal_canonical_tag
                       SET tag_name = UPPER(tag_name),
                           key = UPPER(tag_name) || ':' || canonical_value
                       WHERE tag_name != UPPER(tag_name)"#,
                    [],
                )?;

                // signal_tag_canonicity: cleared and rebuilt each computation cycle anyway.
                conn.execute("DELETE FROM signal_tag_canonicity", [])?;

                // signal_missing_tag: cleared and rebuilt each computation cycle anyway.
                conn.execute("DELETE FROM signal_missing_tag", [])?;

                // signal_compound_tag: data BLOBs contain tag_name in bincode.
                // Clear and re-seed dirty inodes so compound tag detection rebuilds them.
                conn.execute("DELETE FROM signal_compound_tag", [])?;
                seed_dirty_inodes_for(db, "compound_tag")?;

                Ok(())
            },
        });

        // v8→v9: Add has_pictures column to audio_info for embedded album art detection
        registry.register(Migration {
            from_version: 8,
            to_version: 9,
            description: "Add has_pictures column to audio_info for embedded album art detection at index time",
            apply: |db| {
                let has_column: bool = db.conn().query_row(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('audio_info') WHERE name = 'has_pictures'",
                    [],
                    |row| row.get(0),
                )?;

                if !has_column {
                    db.conn().execute(
                        "ALTER TABLE audio_info ADD COLUMN has_pictures INTEGER NOT NULL DEFAULT 0",
                        [],
                    )?;
                }

                Ok(())
            },
        });

        // v9→v10: Rename files.source column to files.zone
        //
        // Disambiguates "zone" (where a file lives: corpus/library/inbox)
        // from "source" (provenance: bandcamp/indie/etc). This is a pure
        // rename with no data transformation.
        registry.register(Migration {
            from_version: 9,
            to_version: 10,
            description: "Rename files.source column to files.zone (zone vs provenance disambiguation)",
            apply: |db| {
                let conn = db.conn();

                // Check if column is already named 'zone' (idempotent)
                let has_zone: bool = conn.query_row(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('files') WHERE name = 'zone'",
                    [],
                    |row| row.get(0),
                )?;

                if !has_zone {
                    // SQLite >= 3.25 supports ALTER TABLE RENAME COLUMN
                    conn.execute(
                        "ALTER TABLE files RENAME COLUMN source TO zone",
                        [],
                    )?;
                }

                // Recreate index with new name (old index on 'source' column
                // will now reference 'zone' after rename, but has stale name)
                conn.execute("DROP INDEX IF EXISTS idx_files_source", [])?;
                conn.execute(
                    "CREATE INDEX IF NOT EXISTS idx_files_zone ON files(zone)",
                    [],
                )?;

                Ok(())
            },
        });

        // v10→v11: Create inbox signal tables
        registry.register(Migration {
            from_version: 10,
            to_version: 11,
            description: "Create inbox signal tables (file_in_inbox, inbox_unindexed, inbox_healthy)",
            apply: |db| {
                use crate::meta::signals::store::CorpusSignalStore;
                use crate::meta::signals::data::{FileInInboxSignal, InboxUnindexedSignal, InboxHealthySignal};
                db.conn().execute_batch(FileInInboxSignal::TABLE_SQL)?;
                db.conn().execute_batch(InboxUnindexedSignal::TABLE_SQL)?;
                db.conn().execute_batch(InboxHealthySignal::TABLE_SQL)?;
                Ok(())
            },
        });

        // v11→v12: Add zone columns to signal_moved_file for cross-zone move detection
        registry.register(Migration {
            from_version: 11,
            to_version: 12,
            description: "Add old_zone/new_zone columns to signal_moved_file for cross-zone move detection",
            apply: |db| {
                db.conn().execute_batch(
                    "ALTER TABLE signal_moved_file ADD COLUMN old_zone TEXT NOT NULL DEFAULT 'corpus';
                     ALTER TABLE signal_moved_file ADD COLUMN new_zone TEXT NOT NULL DEFAULT 'corpus';"
                )?;
                Ok(())
            },
        });

        // v12→v13: Create inbox_corpus_match signal table
        registry.register(Migration {
            from_version: 12,
            to_version: 13,
            description: "Create signal_inbox_corpus_match table for inbox duplicate detection",
            apply: |db| {
                use crate::meta::signals::store::CorpusSignalStore;
                use crate::meta::signals::data::InboxCorpusMatchSignal;
                db.conn().execute_batch(InboxCorpusMatchSignal::TABLE_SQL)?;
                Ok(())
            },
        });

        // v13→v14: Create inbox_tag_canonicity signal table
        registry.register(Migration {
            from_version: 13,
            to_version: 14,
            description: "Create signal_inbox_tag_canonicity table for inbox tag canonicity detection",
            apply: |db| {
                use crate::meta::signals::store::AggregateSignalStore;
                use crate::meta::signals::data::InboxTagCanonicitySignal;
                db.conn().execute_batch(InboxTagCanonicitySignal::TABLE_SQL)?;
                Ok(())
            },
        });

        // v14→v15: Add BLOB data hashes, observation generations, and shit_format dirty inodes
        registry.register(Migration {
            from_version: 14,
            to_version: 15,
            description: "Add data_hash columns for hash-based reconciliation, observation generation columns, and shit_format dirty inode seeding",
            apply: |db| {
                let conn = db.conn();

                // Fix 2: data_hash columns on BLOB signal tables
                for table in &[
                    "signal_fingerprint_overlap",
                    "signal_metadata_duplicate",
                    "signal_duplicate_inode",
                    "signal_missing_tag",
                    "signal_missing_album_single",
                    "signal_tag_canonicity",
                    "signal_inconsistent_album_artist",
                    "signal_cross_source_overlap",
                    "signal_redundant_duplicate",
                    "signal_embeddable_album_art",
                    "signal_deploy_conflict",
                    "signal_inbox_tag_canonicity",
                    "signal_subpar_duplicate",
                    "signal_compound_tag",
                    "signal_oob_tag_sync",
                    "signal_oob_tag_conflict",
                    "signal_inbox_corpus_match",
                ] {
                    add_column_if_missing(conn, table, "data_hash", "INTEGER NOT NULL DEFAULT 0")?;
                }

                // Fix 4: observation generation columns
                add_column_if_missing(conn, "signal_file_in_corpus", "generation", "INTEGER NOT NULL DEFAULT 0")?;
                add_column_if_missing(conn, "signal_file_in_inbox", "generation", "INTEGER NOT NULL DEFAULT 0")?;

                // Fix 3: seed dirty inodes for shit_format initial population
                seed_dirty_inodes_for(db, "shit_format")?;

                Ok(())
            },
        });

        // v15→v16: Create signal_embedded_disc_number table
        registry.register(Migration {
            from_version: 15,
            to_version: 16,
            description: "Create signal_embedded_disc_number table for embedded disc number detection",
            apply: |db| {
                use crate::meta::signals::store::AggregateSignalStore;
                use crate::meta::signals::data::EmbeddedDiscNumberSignal;
                db.conn().execute_batch(EmbeddedDiscNumberSignal::TABLE_SQL)?;
                Ok(())
            },
        });

        // v16→v17: Create inbox missing tag and compound tag signal tables
        registry.register(Migration {
            from_version: 16,
            to_version: 17,
            description: "Create inbox_missing_tag and inbox_compound_tag signal tables",
            apply: |db| {
                use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
                use crate::meta::signals::data::{InboxMissingTagSignal, InboxCompoundTagSignal};
                db.conn().execute_batch(InboxMissingTagSignal::TABLE_SQL)?;
                db.conn().execute_batch(InboxCompoundTagSignal::TABLE_SQL)?;
                Ok(())
            },
        });

        // v17→v18: Create signal_path_tag_mismatch table
        registry.register(Migration {
            from_version: 17,
            to_version: 18,
            description: "Create signal_path_tag_mismatch table for filename-tag schema mismatch detection",
            apply: |db| {
                use crate::meta::signals::store::CorpusSignalStore;
                use crate::meta::signals::data::PathTagMismatchSignal;
                db.conn().execute_batch(PathTagMismatchSignal::TABLE_SQL)?;
                Ok(())
            },
        });

        // v18→v19: Create external matching tables (external_matches, external_no_match, external_retry)
        registry.register(Migration {
            from_version: 18,
            to_version: 19,
            description: "Create external matching tables for AcoustID lookup results",
            apply: |db| {
                db.conn().execute_batch(
                    r#"
                    CREATE TABLE IF NOT EXISTS external_matches (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        inode INTEGER NOT NULL,
                        fingerprint BLOB NOT NULL,
                        source INTEGER NOT NULL,
                        recording_id TEXT NOT NULL,
                        confidence REAL NOT NULL,
                        raw_response BLOB,
                        fetched_at INTEGER NOT NULL,
                        UNIQUE(inode, source, recording_id)
                    );
                    CREATE INDEX IF NOT EXISTS idx_external_matches_inode ON external_matches(inode);
                    CREATE INDEX IF NOT EXISTS idx_external_matches_fp ON external_matches(fingerprint);

                    CREATE TABLE IF NOT EXISTS external_no_match (
                        fingerprint BLOB NOT NULL,
                        source INTEGER NOT NULL,
                        queried_at INTEGER NOT NULL,
                        PRIMARY KEY (fingerprint, source)
                    );

                    CREATE TABLE IF NOT EXISTS external_retry (
                        inode INTEGER NOT NULL,
                        fingerprint BLOB NOT NULL,
                        source INTEGER NOT NULL,
                        failed_at INTEGER NOT NULL,
                        error TEXT,
                        retry_count INTEGER NOT NULL DEFAULT 0,
                        PRIMARY KEY (inode, source)
                    );
                    "#
                )?;
                Ok(())
            },
        });

        // v19→v20: Create signal_external_match table for AcoustID match derivation
        registry.register(Migration {
            from_version: 19,
            to_version: 20,
            description: "Create signal_external_match table for external match signal derivation",
            apply: |db| {
                use crate::meta::signals::store::CorpusSignalStore;
                use crate::meta::signals::data::ExternalMatchSignal;
                db.conn().execute_batch(ExternalMatchSignal::TABLE_SQL)?;
                Ok(())
            },
        });

        registry
    }

    /// Register a migration.
    fn register(&mut self, migration: Migration) {
        self.migrations.push(migration);
    }

    /// Get the latest schema version defined by migrations.
    pub fn latest_version(&self) -> u32 {
        self.migrations
            .last()
            .map(|m| m.to_version)
            .unwrap_or(1) // Base version if no migrations
    }

    /// Check if migrations are needed for the given database.
    pub fn needs_migration(&self, read_db: &ReadOnlyDb<'_>) -> bool {
        let current = read_db.get_schema_version().unwrap_or(1);
        current < self.latest_version()
    }

    /// Get pending migrations for the current database version.
    pub fn pending_migrations(&self, current_version: u32) -> Vec<&Migration> {
        self.migrations
            .iter()
            .filter(|m| m.from_version >= current_version)
            .collect()
    }

    /// Get descriptions of pending migrations.
    pub fn pending_descriptions(&self, read_db: &ReadOnlyDb<'_>) -> Vec<String> {
        let current = read_db.get_schema_version().unwrap_or(1);
        self.pending_migrations(current)
            .iter()
            .map(|m| format!("v{} → v{}: {}", m.from_version, m.to_version, m.description))
            .collect()
    }

    /// Apply a specific migration by ID.
    ///
    /// The migration SQL and schema version update run in a single transaction.
    pub fn apply_migration(
        &self,
        db: &Database,
        migration_id: u32,
        _witness: &MaintenanceWitness,
    ) -> Result<()> {
        let migration = self
            .migrations
            .iter()
            .find(|m| m.to_version == migration_id)
            .context(format!("Migration {} not found", migration_id))?;

        // Run migration and schema version update in a transaction
        db.conn().execute("BEGIN IMMEDIATE", [])?;

        if let Err(e) = (migration.apply)(db) {
            let _ = db.conn().execute("ROLLBACK", []);
            return Err(e);
        }

        if let Err(e) = db.set_schema_version(migration_id) {
            let _ = db.conn().execute("ROLLBACK", []);
            return Err(e.context("Failed to update schema version"));
        }

        db.conn().execute("COMMIT", [])?;
        Ok(())
    }
}

impl Default for MigrationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_registry_baseline() {
        let registry = MigrationRegistry::new();
        let latest = registry.latest_version();

        // Derived from latest_version so this test stays correct as migrations are added.
        assert!(latest >= 16, "expected at least version 16, got {}", latest);
        let migration_count = (latest - 1) as usize; // v1 is base, each migration bumps by 1
        assert_eq!(registry.pending_migrations(1).len(), migration_count);
        assert_eq!(registry.pending_migrations(latest - 1).len(), 1);
        assert!(registry.pending_migrations(latest).is_empty());
    }
}
