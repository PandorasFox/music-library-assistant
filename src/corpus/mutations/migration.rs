//! Database schema migration system.
//!
//! Provides versioned, forward-only migrations that run during startup heartbeat.
//! Each migration transforms the schema from one version to the next.
//!
//! ## Version Tracking
//!
//! Schema version is stored in the `db_admin` table:
//! ```sql
//! CREATE TABLE db_admin (
//!     key TEXT PRIMARY KEY,
//!     value TEXT NOT NULL,
//!     updated_at TEXT NOT NULL DEFAULT (datetime('now'))
//! );
//! ```
//!
//! ## Adding Migrations
//!
//! 1. Add new `Migration` entry in `MigrationRegistry::new()`
//! 2. Increment `MLA_VERSION` in `src/main.rs` (e.g., "alpha-2" → "alpha-3")
//! 3. The migration will automatically run on next startup

use anyhow::{Context, Result};

use crate::corpus::db::Database;

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

        // Version 2 -> 3: Add track_tags table for arbitrary tag storage
        registry.register(Migration {
            from_version: 2,
            to_version: 3,
            description: "Add track_tags table for arbitrary tag storage",
            apply: |db| {
                db.execute_batch(
                    r#"
                    -- Create track_tags table for arbitrary tag storage
                    CREATE TABLE IF NOT EXISTS track_tags (
                        id INTEGER PRIMARY KEY,
                        track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
                        tag_name TEXT NOT NULL,
                        tag_value TEXT NOT NULL,
                        source TEXT NOT NULL DEFAULT 'disk',
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                        UNIQUE(track_id, tag_name)
                    );

                    CREATE INDEX IF NOT EXISTS idx_track_tags_track ON track_tags(track_id);
                    CREATE INDEX IF NOT EXISTS idx_track_tags_name ON track_tags(tag_name);

                    -- Migrate existing tag columns to track_tags
                    INSERT OR IGNORE INTO track_tags (track_id, tag_name, tag_value, source)
                    SELECT id, 'title', title, 'disk' FROM tracks WHERE title IS NOT NULL AND title != '';

                    INSERT OR IGNORE INTO track_tags (track_id, tag_name, tag_value, source)
                    SELECT id, 'artist', artist, 'disk' FROM tracks WHERE artist IS NOT NULL AND artist != '';

                    INSERT OR IGNORE INTO track_tags (track_id, tag_name, tag_value, source)
                    SELECT id, 'album', album, 'disk' FROM tracks WHERE album IS NOT NULL AND album != '';

                    INSERT OR IGNORE INTO track_tags (track_id, tag_name, tag_value, source)
                    SELECT id, 'album_artist', album_artist, 'disk' FROM tracks WHERE album_artist IS NOT NULL AND album_artist != '';

                    INSERT OR IGNORE INTO track_tags (track_id, tag_name, tag_value, source)
                    SELECT id, 'genre', genre, 'disk' FROM tracks WHERE genre IS NOT NULL AND genre != '';

                    INSERT OR IGNORE INTO track_tags (track_id, tag_name, tag_value, source)
                    SELECT id, 'isrc', isrc, 'disk' FROM tracks WHERE isrc IS NOT NULL AND isrc != '';

                    INSERT OR IGNORE INTO track_tags (track_id, tag_name, tag_value, source)
                    SELECT id, 'track_number', CAST(track_number AS TEXT), 'disk' FROM tracks WHERE track_number IS NOT NULL;
                    "#,
                )?;

                // Update schema version using the proper table
                db.set_schema_version(3)
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
            .unwrap_or(2) // Base version if no migrations
    }

    /// Check if migrations are needed for the given database.
    pub fn needs_migration(&self, db: &Database) -> bool {
        let current = db.get_schema_version().unwrap_or(2);
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
        let current = db.get_schema_version().unwrap_or(2);
        self.pending_migrations(current)
            .iter()
            .map(|m| format!("v{} → v{}: {}", m.from_version, m.to_version, m.description))
            .collect()
    }

    /// Apply a specific migration by ID.
    pub fn apply_migration(&self, db: &Database, migration_id: u32) -> Result<()> {
        let migration = self
            .migrations
            .iter()
            .find(|m| m.to_version == migration_id)
            .context(format!("Migration {} not found", migration_id))?;

        (migration.apply)(db)
    }

    /// Apply all pending migrations.
    ///
    /// Returns the number of migrations applied.
    pub fn apply_all_pending(&self, db: &Database) -> Result<usize> {
        let current = db.get_schema_version().unwrap_or(2);
        let pending = self.pending_migrations(current);
        let count = pending.len();

        for migration in pending {
            // Note: Consider adding logging here if needed
            // For now, migrations are applied silently
            (migration.apply)(db)?;
        }

        Ok(count)
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
    fn test_migration_registry() {
        let registry = MigrationRegistry::new();

        // Should have at least the track_tags migration
        assert!(registry.latest_version() >= 3);

        // Pending from version 2 should include v2->v3
        let pending = registry.pending_migrations(2);
        assert!(!pending.is_empty());
        assert!(pending.iter().any(|m| m.to_version == 3));

        // Pending from version 3 should be empty
        let pending_from_3 = registry.pending_migrations(3);
        assert!(pending_from_3.is_empty());
    }
}
