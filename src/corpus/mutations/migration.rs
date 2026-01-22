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
use rusqlite::params;

use crate::config;
use crate::corpus::db::Database;
use crate::witch::MigrationWitness;

/// Migration function: Convert fingerprints from TEXT to BLOB format.
///
/// This migration:
/// 1. Creates a new fingerprint_blob column (if not exists)
/// 2. Converts existing TEXT fingerprints to binary BLOB
/// 3. Updates fingerprint_dup signals to remove redundant fingerprint from metadata
/// 4. Drops old TEXT column and renames BLOB column
///
/// Expected savings: ~400MB for a 1GB database (fingerprint data + index)
///
/// This migration is idempotent - it can recover from partial failures.
fn migrate_fingerprints_to_blob(db: &Database) -> Result<()> {
    use crate::config::log_message;

    let _ = log_message("[MIGRATION v3→v4] Starting fingerprint TEXT→BLOB conversion");

    // Check if we're recovering from a partial migration
    let has_blob_column: bool = db.conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('tracks') WHERE name = 'fingerprint_blob'",
        params![],
        |row| row.get(0),
    ).unwrap_or(false);

    // Step 1: Add new BLOB column (skip if recovering)
    if !has_blob_column {
        db.execute_batch(
            "ALTER TABLE tracks ADD COLUMN fingerprint_blob BLOB;"
        )?;
    }

    // Step 2: Convert existing fingerprints in batches
    // Read all track IDs with TEXT fingerprints that haven't been converted yet
    let mut stmt = db.conn.prepare(
        "SELECT id, fingerprint FROM tracks WHERE fingerprint IS NOT NULL AND fingerprint != '' AND fingerprint_blob IS NULL"
    )?;

    let tracks: Vec<(i64, String)> = stmt
        .query_map(params![], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let total = tracks.len();
    if total > 0 {
        let _ = log_message(&format!("[MIGRATION v3→v4] Converting {} fingerprints", total));
    }

    // Convert in batches
    let batch_size = 1000;
    for chunk in tracks.chunks(batch_size) {
        for (track_id, fp_text) in chunk {
            let blob: Vec<u8> = fp_text
                .split(',')
                .filter_map(|s| s.trim().parse::<u32>().ok())
                .flat_map(|n| n.to_le_bytes())
                .collect();

            if !blob.is_empty() {
                db.conn.execute(
                    "UPDATE tracks SET fingerprint_blob = ?1 WHERE id = ?2",
                    params![blob, track_id],
                )?;
            }
        }
    }

    // Step 3: Update fingerprint_dup signals to remove redundant fingerprint from metadata
    let mut signal_stmt = db.conn.prepare(
        "SELECT id, metadata_json FROM signals WHERE issue_type = 'fingerprint_dup' AND metadata_json IS NOT NULL"
    )?;

    let signals: Vec<(i64, String)> = signal_stmt
        .query_map(params![], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    for (signal_id, metadata_json) in signals {
        if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&metadata_json) {
            if let Some(obj) = json.as_object_mut() {
                obj.remove("fingerprint");
                if let Ok(new_json) = serde_json::to_string(&json) {
                    db.conn.execute(
                        "UPDATE signals SET metadata_json = ?1 WHERE id = ?2",
                        params![new_json, signal_id],
                    )?;
                }
            }
        }
    }

    // Step 4: Schema changes - drop old column, rename new, recreate index
    let _ = log_message("[MIGRATION v3→v4] Finalizing schema changes");

    // Check if we've already completed the swap (tracks_new doesn't exist, fingerprint is BLOB)
    let tracks_new_exists: bool = db.conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='tracks_new'",
        params![],
        |row| row.get(0),
    ).unwrap_or(false);

    // Check fingerprint column type - if it's already BLOB, we're done
    let fp_type: String = db.conn.query_row(
        "SELECT type FROM pragma_table_info('tracks') WHERE name = 'fingerprint'",
        params![],
        |row| row.get(0),
    ).unwrap_or_else(|_| "TEXT".to_string());

    if fp_type == "BLOB" && !tracks_new_exists {
        let _ = log_message("[MIGRATION v3→v4] Table already migrated, skipping swap");
    } else {
        // Need to do the table swap
        // Foreign keys must be off for the entire connection, not just the transaction
        db.conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        db.conn.execute_batch("DROP INDEX IF EXISTS idx_fingerprint;")?;

        db.conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS tracks_new (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                source TEXT NOT NULL,
                inode INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                file_type TEXT NOT NULL,
                duration_ms INTEGER,
                bitrate_kbps INTEGER,
                sample_rate INTEGER,
                fingerprint BLOB,
                scanned_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
        "#)?;

        // Copy data - check if tracks_new already has data (recovery case)
        let tracks_new_count: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM tracks_new",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        if tracks_new_count == 0 {
            db.conn.execute_batch(r#"
                INSERT INTO tracks_new (id, path, source, inode, file_size, file_type,
                                        duration_ms, bitrate_kbps, sample_rate, fingerprint, scanned_at)
                SELECT id, path, source, inode, file_size, file_type,
                       duration_ms, bitrate_kbps, sample_rate, fingerprint_blob, scanned_at
                FROM tracks;
            "#)?;
        }

        // Swap tables
        db.conn.execute_batch("DROP TABLE tracks;")?;
        db.conn.execute_batch("ALTER TABLE tracks_new RENAME TO tracks;")?;

        // Recreate indexes
        db.conn.execute_batch(r#"
            CREATE INDEX IF NOT EXISTS idx_source ON tracks(source);
            CREATE INDEX IF NOT EXISTS idx_inode ON tracks(inode);
            CREATE INDEX IF NOT EXISTS idx_duration ON tracks(duration_ms);
            CREATE INDEX IF NOT EXISTS idx_fingerprint ON tracks(fingerprint);
        "#)?;

        db.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let _ = log_message("[MIGRATION v3→v4] Table swap complete");
    }

    db.set_schema_version(4)?;
    let _ = log_message("[MIGRATION v3→v4] Complete - fingerprints now stored as BLOB");

    Ok(())
}

/// Migration function: Convert absolute paths to relative paths.
///
/// This migration converts all file paths in the database from absolute paths
/// to paths relative to their respective roots (corpus_root, libraries_root, legacy_library).
///
/// After this migration, the archive can be relocated by simply updating the config
/// file with the new root paths.
///
/// Tables affected:
/// - tracks.path: relative to corpus_root (source="corpus") or legacy_library (source="legacy")
/// - scan_state.path: relative to source root
/// - deployment_log.corpus_path: relative to corpus_root
/// - deployment_log.deployed_path: relative to libraries_root
/// - library_scan_state.file_path: relative to libraries_root
/// - library_scan_state.library_root: removed (redundant with library_name)
/// - signals.issue_key: paths become relative (signal_type determines root)
fn migrate_to_relative_paths(db: &Database) -> Result<()> {
    use config::log_message;

    let _ = log_message("[MIGRATION v4→v5] Starting absolute→relative path conversion");

    // Load config to get current roots
    let cfg = config::load_config().context(
        "Failed to load config.kdl. Path migration requires the config file with correct roots.\n\
         If you have relocated your archive, please restore the original config temporarily,\n\
         run MLA to complete migration, then update config with new paths."
    )?;

    let corpus_root = cfg.corpus_root.to_string_lossy();
    let libraries_root = cfg.libraries_root.to_string_lossy();
    let legacy_root = cfg.legacy_library.as_ref().map(|p| p.to_string_lossy().into_owned());

    let _ = log_message(&format!("[MIGRATION v4→v5] corpus_root: {}", corpus_root));
    let _ = log_message(&format!("[MIGRATION v4→v5] libraries_root: {}", libraries_root));
    if let Some(ref legacy) = legacy_root {
        let _ = log_message(&format!("[MIGRATION v4→v5] legacy_library: {}", legacy));
    }

    // Helper to strip prefix and return relative path
    fn strip_prefix(path: &str, prefix: &str) -> Option<String> {
        let normalized_prefix = prefix.trim_end_matches('/');
        path.strip_prefix(normalized_prefix)
            .and_then(|rest| rest.strip_prefix('/'))
            .map(|s| s.to_string())
    }

    // =========================================================================
    // 1. Convert tracks.path
    // =========================================================================
    let _ = log_message("[MIGRATION v4→v5] Converting tracks.path");

    // Corpus tracks
    let corpus_pattern = format!("{}/%", corpus_root.trim_end_matches('/'));
    let corpus_updated: usize = db.conn.execute(
        "UPDATE tracks SET path = substr(path, ?1 + 2) WHERE source = 'corpus' AND path LIKE ?2 ESCAPE '\\'",
        params![corpus_root.len() as i64, corpus_pattern],
    )?;
    let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} corpus track paths", corpus_updated));

    // Legacy tracks (if legacy_library configured)
    if let Some(ref legacy) = legacy_root {
        let legacy_pattern = format!("{}/%", legacy.trim_end_matches('/'));
        let legacy_updated: usize = db.conn.execute(
            "UPDATE tracks SET path = substr(path, ?1 + 2) WHERE source = 'legacy' AND path LIKE ?2 ESCAPE '\\'",
            params![legacy.len() as i64, legacy_pattern],
        )?;
        let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} legacy track paths", legacy_updated));
    }

    // =========================================================================
    // 2. Convert scan_state.path
    // =========================================================================
    let _ = log_message("[MIGRATION v4→v5] Converting scan_state.path");

    // Corpus scan state
    let corpus_scan_updated: usize = db.conn.execute(
        "UPDATE scan_state SET path = substr(path, ?1 + 2) WHERE source = 'corpus' AND path LIKE ?2 ESCAPE '\\'",
        params![corpus_root.len() as i64, corpus_pattern],
    )?;
    let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} corpus scan_state paths", corpus_scan_updated));

    // Legacy scan state
    if let Some(ref legacy) = legacy_root {
        let legacy_pattern = format!("{}/%", legacy.trim_end_matches('/'));
        let legacy_scan_updated: usize = db.conn.execute(
            "UPDATE scan_state SET path = substr(path, ?1 + 2) WHERE source = 'legacy' AND path LIKE ?2 ESCAPE '\\'",
            params![legacy.len() as i64, legacy_pattern],
        )?;
        let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} legacy scan_state paths", legacy_scan_updated));
    }

    // Library scan state (any source that's not corpus or legacy)
    let lib_pattern = format!("{}/%", libraries_root.trim_end_matches('/'));
    let lib_scan_updated: usize = db.conn.execute(
        "UPDATE scan_state SET path = substr(path, ?1 + 2) WHERE source NOT IN ('corpus', 'legacy') AND path LIKE ?2 ESCAPE '\\'",
        params![libraries_root.len() as i64, lib_pattern],
    )?;
    let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} library scan_state paths", lib_scan_updated));

    // =========================================================================
    // 3. Convert deployment_log paths
    // =========================================================================
    let _ = log_message("[MIGRATION v4→v5] Converting deployment_log paths");

    let deploy_corpus_updated: usize = db.conn.execute(
        "UPDATE deployment_log SET corpus_path = substr(corpus_path, ?1 + 2) WHERE corpus_path LIKE ?2 ESCAPE '\\'",
        params![corpus_root.len() as i64, corpus_pattern],
    )?;
    let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} deployment_log corpus_path entries", deploy_corpus_updated));

    let deploy_lib_updated: usize = db.conn.execute(
        "UPDATE deployment_log SET deployed_path = substr(deployed_path, ?1 + 2) WHERE deployed_path LIKE ?2 ESCAPE '\\'",
        params![libraries_root.len() as i64, lib_pattern],
    )?;
    let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} deployment_log deployed_path entries", deploy_lib_updated));

    // =========================================================================
    // 4. Convert library_scan_state paths
    // =========================================================================
    let _ = log_message("[MIGRATION v4→v5] Converting library_scan_state paths");

    let lib_scan_file_updated: usize = db.conn.execute(
        "UPDATE library_scan_state SET file_path = substr(file_path, ?1 + 2) WHERE file_path LIKE ?2 ESCAPE '\\'",
        params![libraries_root.len() as i64, lib_pattern],
    )?;
    let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} library_scan_state file_path entries", lib_scan_file_updated));

    // Also convert library_root to relative (or we could drop it, but let's keep it relative for now)
    let lib_root_updated: usize = db.conn.execute(
        "UPDATE library_scan_state SET library_root = substr(library_root, ?1 + 2) WHERE library_root LIKE ?2 ESCAPE '\\'",
        params![libraries_root.len() as i64, lib_pattern],
    )?;
    let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} library_scan_state library_root entries", lib_root_updated));

    // =========================================================================
    // 5. Convert signals.issue_key for file-based signals
    // =========================================================================
    let _ = log_message("[MIGRATION v4→v5] Converting signal keys");

    // Corpus signals: file_in_corpus, unindexed_file, missing_file, healthy_file
    let corpus_signal_types = vec!["file_in_corpus", "unindexed_file", "missing_file", "healthy_file"];
    for signal_type in &corpus_signal_types {
        let updated: usize = db.conn.execute(
            "UPDATE signals SET issue_key = substr(issue_key, ?1 + 2) WHERE issue_type = ?2 AND issue_key LIKE ?3 ESCAPE '\\'",
            params![corpus_root.len() as i64, signal_type, corpus_pattern],
        )?;
        if updated > 0 {
            let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} {} signal keys", updated, signal_type));
        }
    }

    // Library signals: library_stale, library_leftover
    let library_signal_types = vec!["library_stale", "library_leftover"];
    for signal_type in &library_signal_types {
        let updated: usize = db.conn.execute(
            "UPDATE signals SET issue_key = substr(issue_key, ?1 + 2) WHERE issue_type = ?2 AND issue_key LIKE ?3 ESCAPE '\\'",
            params![libraries_root.len() as i64, signal_type, lib_pattern],
        )?;
        if updated > 0 {
            let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} {} signal keys", updated, signal_type));
        }
    }

    // Deploy conflict: key format is "deploy_conflict:{library_name}:{deploy_path}"
    // Need to update the deploy_path part within the key
    let mut conflict_stmt = db.conn.prepare(
        "SELECT id, issue_key FROM signals WHERE issue_type = 'deploy_conflict'"
    )?;
    let conflicts: Vec<(i64, String)> = conflict_stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    drop(conflict_stmt);

    let mut conflict_updated = 0;
    for (id, key) in conflicts {
        // Key format: deploy_conflict:{library_name}:{deploy_path}
        // or just the deploy_path directly in newer code
        if let Some(rel_key) = strip_prefix(&key, &libraries_root) {
            db.conn.execute(
                "UPDATE signals SET issue_key = ?1 WHERE id = ?2",
                params![rel_key, id],
            )?;
            conflict_updated += 1;
        }
    }
    if conflict_updated > 0 {
        let _ = log_message(&format!("[MIGRATION v4→v5] Updated {} deploy_conflict signal keys", conflict_updated));
    }

    // Also update metadata_json for signals that contain paths
    // (fingerprint_dup has track_ids, not paths, so skip)
    // (deploy_conflict metadata has target_path and conflicting_paths)
    let _ = log_message("[MIGRATION v4→v5] Updating signal metadata paths");

    let mut metadata_stmt = db.conn.prepare(
        "SELECT id, metadata_json FROM signals WHERE issue_type = 'deploy_conflict' AND metadata_json IS NOT NULL"
    )?;
    let metadata_signals: Vec<(i64, String)> = metadata_stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    drop(metadata_stmt);

    let corpus_root_owned = corpus_root.to_string();
    let libraries_root_owned = libraries_root.to_string();

    for (id, json_str) in metadata_signals {
        if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&json_str) {
            let mut modified = false;

            // Update target_path (library path)
            if let Some(serde_json::Value::String(target_path)) = json.get("target_path") {
                if let Some(rel) = strip_prefix(target_path, &libraries_root_owned) {
                    json["target_path"] = serde_json::Value::String(rel);
                    modified = true;
                }
            }

            // Update conflicting_paths (corpus paths)
            if let Some(serde_json::Value::Array(paths)) = json.get_mut("conflicting_paths") {
                for path in paths.iter_mut() {
                    if let serde_json::Value::String(p) = path {
                        if let Some(rel) = strip_prefix(p, &corpus_root_owned) {
                            *path = serde_json::Value::String(rel);
                            modified = true;
                        }
                    }
                }
            }

            if modified {
                if let Ok(new_json) = serde_json::to_string(&json) {
                    db.conn.execute(
                        "UPDATE signals SET metadata_json = ?1 WHERE id = ?2",
                        params![new_json, id],
                    )?;
                }
            }
        }
    }

    // =========================================================================
    // 6. Update schema version
    // =========================================================================
    db.set_schema_version(5)?;
    let _ = log_message("[MIGRATION v4→v5] Complete - paths are now stored relative to roots");

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

        // v1 → v2: Add library_scan_state table for phase-stratified computation data flow
        registry.register(Migration {
            from_version: 1,
            to_version: 2,
            description: "Add library_scan_state table for phase-stratified computations",
            apply: |db| {
                db.execute_batch(
                    r#"
                    -- Library scan state: stores library file scan results between phases
                    -- Written by ScanLibraryDirectory (Awakening), read by DeriveDeployHealthSignals (Awake)
                    CREATE TABLE IF NOT EXISTS library_scan_state (
                        id INTEGER PRIMARY KEY,
                        library_name TEXT NOT NULL,
                        library_root TEXT NOT NULL,
                        file_path TEXT NOT NULL,
                        inode INTEGER NOT NULL,
                        scanned_at INTEGER NOT NULL,
                        UNIQUE(library_name, file_path)
                    );
                    CREATE INDEX IF NOT EXISTS idx_library_scan_library ON library_scan_state(library_name);
                    CREATE INDEX IF NOT EXISTS idx_library_scan_root ON library_scan_state(library_root);
                    "#,
                )?;
                db.set_schema_version(2)?;
                Ok(())
            },
        });

        // v2 → v3: Drop legacy tables, signals table exists in base schema
        // Obliterates "health issues" terminology - signals table is authoritative
        registry.register(Migration {
            from_version: 2,
            to_version: 3,
            description: "Drop health_issues and tag_canonicalization tables",
            apply: |db| {
                db.execute_batch(
                    r#"
                    -- Drop legacy health_issues table (signals table in base schema is authoritative)
                    DROP TABLE IF EXISTS health_issues;
                    DROP INDEX IF EXISTS idx_health_issues_type;
                    DROP INDEX IF EXISTS idx_health_issues_discovered;

                    -- Drop tag_canonicalization table (canonicity now uses signals)
                    DROP TABLE IF EXISTS tag_canonicalization;
                    "#,
                )?;
                db.set_schema_version(3)?;
                Ok(())
            },
        });

        // v3 → v4: Convert fingerprints from TEXT to BLOB for ~70% DB size reduction
        // Also removes redundant fingerprint from fingerprint_dup signal metadata
        registry.register(Migration {
            from_version: 3,
            to_version: 4,
            description: "Convert fingerprints to binary BLOB format",
            apply: migrate_fingerprints_to_blob,
        });

        // v4 → v5: Convert absolute paths to relative paths
        // Enables archive relocation without database modification
        registry.register(Migration {
            from_version: 4,
            to_version: 5,
            description: "Convert paths from absolute to relative for portability",
            apply: migrate_to_relative_paths,
        });

        registry
    }

    /// Get fingerprint as text from BLOB for signal key compatibility.
    /// Used when we need to create signal keys that match the old text format.
    pub fn fingerprint_blob_to_text(blob: &[u8]) -> String {
        // BLOB is stored as little-endian u32 values
        blob.chunks_exact(4)
            .map(|chunk| {
                let val = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                val.to_string()
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Convert fingerprint text to BLOB format.
    pub fn fingerprint_text_to_blob(text: &str) -> Vec<u8> {
        text.split(',')
            .filter_map(|s| s.trim().parse::<u32>().ok())
            .flat_map(|n| n.to_le_bytes())
            .collect()
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

        // Latest schema version is v5 (after relative paths migration)
        assert_eq!(registry.latest_version(), 5);

        // Four pending migrations from v1
        let pending = registry.pending_migrations(1);
        assert_eq!(pending.len(), 4);
        assert_eq!(pending[0].from_version, 1);
        assert_eq!(pending[0].to_version, 2);
        assert_eq!(pending[1].from_version, 2);
        assert_eq!(pending[1].to_version, 3);
        assert_eq!(pending[2].from_version, 3);
        assert_eq!(pending[2].to_version, 4);
        assert_eq!(pending[3].from_version, 4);
        assert_eq!(pending[3].to_version, 5);

        // Three pending migrations from v2
        let pending_v2 = registry.pending_migrations(2);
        assert_eq!(pending_v2.len(), 3);
        assert_eq!(pending_v2[0].from_version, 2);
        assert_eq!(pending_v2[0].to_version, 3);

        // Two pending migrations from v3
        let pending_v3 = registry.pending_migrations(3);
        assert_eq!(pending_v3.len(), 2);
        assert_eq!(pending_v3[0].from_version, 3);
        assert_eq!(pending_v3[0].to_version, 4);

        // One pending migration from v4
        let pending_v4 = registry.pending_migrations(4);
        assert_eq!(pending_v4.len(), 1);
        assert_eq!(pending_v4[0].from_version, 4);
        assert_eq!(pending_v4[0].to_version, 5);

        // No pending migrations from v5
        let pending_v5 = registry.pending_migrations(5);
        assert!(pending_v5.is_empty());
    }

    #[test]
    fn test_fingerprint_conversion() {
        // Test text to blob
        let text = "100037086,83255630,83256094";
        let blob = MigrationRegistry::fingerprint_text_to_blob(text);

        // Each u32 is 4 bytes, so 3 numbers = 12 bytes
        assert_eq!(blob.len(), 12);

        // Verify little-endian encoding
        let first = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]);
        assert_eq!(first, 100037086);

        // Test blob back to text
        let roundtrip = MigrationRegistry::fingerprint_blob_to_text(&blob);
        assert_eq!(roundtrip, text);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use tempfile::NamedTempFile;

    /// Test full migration flow with sample fingerprint data.
    #[test]
    fn test_migration_v3_to_v4() {
        // Create temp database
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).unwrap();

        // Set up v3 schema with TEXT fingerprints
        db.execute_batch(r#"
            UPDATE app_metadata SET value = '3' WHERE key = 'schema_version';
        "#).unwrap();

        // Add some tracks with TEXT fingerprints
        db.conn.execute(
            "INSERT INTO tracks (path, source, inode, file_size, file_type, fingerprint) VALUES (?1, 'corpus', 1, 1000, 'flac', ?2)",
            params!["/test/track1.flac", "100,200,300,400"],
        ).unwrap();
        db.conn.execute(
            "INSERT INTO tracks (path, source, inode, file_size, file_type, fingerprint) VALUES (?1, 'corpus', 2, 1000, 'flac', ?2)",
            params!["/test/track2.flac", "100,200,300,400"],  // Same fingerprint = duplicate
        ).unwrap();
        db.conn.execute(
            "INSERT INTO tracks (path, source, inode, file_size, file_type, fingerprint) VALUES (?1, 'corpus', 3, 1000, 'flac', ?2)",
            params!["/test/track3.flac", "500,600,700,800"],  // Different fingerprint
        ).unwrap();

        // Add fingerprint_dup signal with redundant fingerprint in metadata
        db.conn.execute(
            r#"INSERT INTO signals (issue_type, issue_key, metadata_json) VALUES ('fingerprint_dup', '100,200,300,400', ?1)"#,
            params![r#"{"fingerprint":"100,200,300,400","track_ids":[1,2]}"#],
        ).unwrap();

        // Verify TEXT fingerprints before migration
        let fp_before: String = db.conn.query_row(
            "SELECT fingerprint FROM tracks WHERE id = 1",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(fp_before, "100,200,300,400");

        // Run migration
        migrate_fingerprints_to_blob(&db).unwrap();

        // Verify version updated
        let version = db.get_schema_version().unwrap();
        assert_eq!(version, 4);

        // Verify fingerprints are now BLOB
        let fp_blob: Vec<u8> = db.conn.query_row(
            "SELECT fingerprint FROM tracks WHERE id = 1",
            [],
            |row| row.get(0),
        ).unwrap();

        // Should be 16 bytes (4 u32s × 4 bytes)
        assert_eq!(fp_blob.len(), 16);

        // Verify correct values
        let values: Vec<u32> = fp_blob
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(values, vec![100, 200, 300, 400]);

        // Verify signal metadata no longer contains fingerprint
        let metadata_json: String = db.conn.query_row(
            "SELECT metadata_json FROM signals WHERE issue_type = 'fingerprint_dup'",
            [],
            |row| row.get(0),
        ).unwrap();

        let json: serde_json::Value = serde_json::from_str(&metadata_json).unwrap();
        assert!(json.get("fingerprint").is_none(), "fingerprint should be removed from metadata");
        assert!(json.get("track_ids").is_some(), "track_ids should remain");
    }
}
