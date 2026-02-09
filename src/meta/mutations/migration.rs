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
//! ## Current State
//!
//! Schema v1 is the inode-based files/audio_info/corpus_tags schema.
//! This is a fresh start - no migrations from previous schemas exist.

use anyhow::{Context, Result};

use crate::corpus::db::Database;
use crate::witch::MigrationWitness;

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
    pub fn needs_migration(&self, db: &Database) -> bool {
        let current = db.get_schema_version().unwrap_or(1);
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
    pub fn pending_descriptions(&self, db: &Database) -> Vec<String> {
        let current = db.get_schema_version().unwrap_or(1);
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
        _witness: &MigrationWitness,
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

        // v1→v2: tags_version + dirty_inodes
        // v2→v3: inode column in signals table
        assert_eq!(registry.latest_version(), 3);
        // From v1, there should be 2 pending migrations
        assert_eq!(registry.pending_migrations(1).len(), 2);
        // From v2, there should be 1 pending migration
        assert_eq!(registry.pending_migrations(2).len(), 1);
        // From v3, no pending migrations
        assert!(registry.pending_migrations(3).is_empty());
    }
}
