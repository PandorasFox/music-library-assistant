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
///
/// Initially empty — all historical data migrations from the old migration
/// system are already baked into the current schema state.
pub fn all_data_migrations() -> Vec<DataMigrationEntry> {
    vec![]
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
