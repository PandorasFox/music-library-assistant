//! Database schema migration system.
//!
//! Provides versioned, forward-only migrations that run at startup.
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
use crate::daemon::MigrationWitness;

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
    ///
    /// Schema is currently at v1 with no migrations needed.
    /// Future migrations would be added here when breaking changes are made.
    pub fn new() -> Self {
        Self {
            migrations: Vec::new(),
        }
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
    /// Requires a `MigrationWitness` to prove this is being called from the daemon
    /// execution context, ensuring migrations cannot be accidentally applied from
    /// arbitrary code paths.
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

        (migration.apply)(db)
    }

    /// Apply all pending migrations.
    ///
    /// Returns the number of migrations applied.
    ///
    /// Requires a `MigrationWitness` to prove this is being called from an
    /// authorized migration context (either daemon execution or startup with
    /// user approval).
    pub fn apply_all_pending(&self, db: &Database, _witness: &MigrationWitness) -> Result<usize> {
        let current = db.get_schema_version().unwrap_or(1);
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

        // Schema is at v1 with no migrations
        assert_eq!(registry.latest_version(), 1);

        // No pending migrations from v1
        let pending = registry.pending_migrations(1);
        assert!(pending.is_empty());

        // needs_migration should return false for v1 database
        // (can't test without actual DB, but pending_migrations covers the logic)
    }
}
