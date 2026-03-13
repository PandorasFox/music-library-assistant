//! Schema reconciliation engine.
//!
//! Compares the expected schema (from `table_schema::schema_inventory()`) against
//! the actual database using SQLite's own introspection (`pragma_table_info`).
//! Produces a `ReconciliationPlan` describing the changes needed, which can be
//! reviewed by the operator and then applied.
//!
//! ## Core trick: temp-DB introspection
//!
//! No SQL parsing needed. We open an in-memory SQLite DB, execute all TABLE_SQL
//! strings against it, then compare `pragma_table_info` between the in-memory
//! (expected) and real (actual) databases. This leverages SQLite's own parser.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use super::data_migrations;
use super::queries::Database;
use super::table_schema::{self, TableEntry, TableKind, FINGERPRINT_KEY};
use crate::witch::MaintenanceWitness;

/// Column metadata from `pragma_table_info`.
#[derive(Debug, Clone)]
struct ColumnInfo {
    name: String,
    col_type: String,
    notnull: bool,
    dflt_value: Option<String>,
    _pk: i32,
}

/// A table that needs to be created from scratch.
#[derive(Debug)]
pub struct NewTable {
    pub name: String,
    pub create_sql: String,
    pub index_sql: Vec<String>,
}

/// A column that needs to be added to an existing Core/Decision table.
#[derive(Debug)]
pub struct ColumnAddition {
    pub table: String,
    pub column: String,
    pub column_def: String,
}

/// A computed signal table that has schema drift and needs DROP+CREATE.
#[derive(Debug)]
pub struct SignalRecreation {
    pub table: String,
    pub create_sql: String,
}

/// An index that needs to be created.
#[derive(Debug)]
pub struct NewIndex {
    pub create_sql: String,
}

/// The full reconciliation plan — everything needed to bring the DB up to date.
pub struct ReconciliationPlan {
    pub new_tables: Vec<NewTable>,
    pub new_columns: Vec<ColumnAddition>,
    pub recreated_signals: Vec<SignalRecreation>,
    pub new_indices: Vec<NewIndex>,
    pub data_migrations: Vec<&'static data_migrations::DataMigrationEntry>,
}

impl ReconciliationPlan {
    /// Compute the reconciliation plan by comparing expected vs actual schema.
    pub fn compute(conn: &Connection) -> Result<Self> {
        let inventory = table_schema::schema_inventory();

        // Open in-memory DB and execute all TABLE_SQL to get expected schema
        let mem_db = Connection::open_in_memory()
            .context("Failed to open in-memory DB for schema introspection")?;

        for entry in &inventory {
            mem_db.execute_batch(entry.create_sql).with_context(|| {
                format!("Failed to create expected table '{}' in memory", entry.name)
            })?;
        }

        // Get actual tables in the real database
        let actual_tables = get_table_names(conn)?;

        let mut plan = ReconciliationPlan {
            new_tables: Vec::new(),
            new_columns: Vec::new(),
            recreated_signals: Vec::new(),
            new_indices: Vec::new(),
            data_migrations: Vec::new(),
        };

        for entry in &inventory {
            if !actual_tables.contains(entry.name) {
                // Table doesn't exist at all — create it
                plan.new_tables.push(NewTable {
                    name: entry.name.to_string(),
                    create_sql: entry.create_sql.to_string(),
                    index_sql: entry.index_sql.iter().map(|s| s.to_string()).collect(),
                });
            } else {
                // Table exists — check for column differences
                let expected_cols = get_column_info(&mem_db, entry.name)?;
                let actual_cols = get_column_info(conn, entry.name)?;

                let actual_col_names: HashSet<&str> =
                    actual_cols.iter().map(|c| c.name.as_str()).collect();

                let missing_cols: Vec<&ColumnInfo> = expected_cols
                    .iter()
                    .filter(|c| !actual_col_names.contains(c.name.as_str()))
                    .collect();

                if !missing_cols.is_empty() {
                    match entry.kind {
                        TableKind::Core | TableKind::Decision => {
                            // ADD COLUMN for each missing column
                            for col in missing_cols {
                                plan.new_columns.push(ColumnAddition {
                                    table: entry.name.to_string(),
                                    column: col.name.clone(),
                                    column_def: build_column_def(col),
                                });
                            }
                        }
                        TableKind::Computed => {
                            // DROP+CREATE for computed tables
                            plan.recreated_signals.push(SignalRecreation {
                                table: entry.name.to_string(),
                                create_sql: entry.create_sql.to_string(),
                            });
                        }
                    }
                }

                // Check for missing indices
                check_missing_indices(conn, entry, &mut plan)?;
            }
        }

        Ok(plan)
    }

    /// Compute plan including data migrations (needs Database for applied-check).
    pub fn compute_full(db: &Database) -> Result<Self> {
        let mut plan = Self::compute(db.conn())?;

        // Check for pending data migrations
        for entry in data_migrations::all_data_migrations().leak() {
            if !data_migrations::is_migration_applied(db, entry.id) {
                plan.data_migrations.push(entry);
            }
        }

        Ok(plan)
    }

    /// Whether the plan has any work to do.
    pub fn is_empty(&self) -> bool {
        self.new_tables.is_empty()
            && self.new_columns.is_empty()
            && self.recreated_signals.is_empty()
            && self.new_indices.is_empty()
            && self.data_migrations.is_empty()
    }

    /// Human-readable descriptions of all planned changes, for the approval UI.
    pub fn descriptions(&self) -> Vec<String> {
        let mut lines = Vec::new();

        for t in &self.new_tables {
            lines.push(format!("Create table: {}", t.name));
        }
        for c in &self.new_columns {
            lines.push(format!("Add column: {}.{}", c.table, c.column));
        }
        for s in &self.recreated_signals {
            lines.push(format!("Recreate signal table: {}", s.table));
        }
        for i in &self.new_indices {
            // Extract a short label from the SQL
            let label = i
                .create_sql
                .split("IF NOT EXISTS ")
                .nth(1)
                .and_then(|s| s.split(' ').next())
                .unwrap_or(&i.create_sql);
            lines.push(format!("Create index: {}", label));
        }
        for m in &self.data_migrations {
            lines.push(format!("Data migration: {}", m.description));
        }

        lines
    }

    /// Execute the reconciliation plan against the database.
    ///
    /// Wraps all DDL changes in a transaction. Re-seeds dirty inodes for
    /// any recreated signal tables so computations re-run.
    pub fn execute(self, db: &Database, _witness: &MaintenanceWitness) -> Result<()> {
        let conn = db.conn();

        conn.execute_batch("BEGIN IMMEDIATE")
            .context("Failed to begin reconciliation transaction")?;

        let result = self.execute_inner(db);

        match result {
            Ok(()) => {
                // Update schema fingerprint
                let fp = table_schema::schema_fingerprint().to_string();
                conn.execute(
                    "INSERT OR REPLACE INTO app_metadata (key, value, updated_at) VALUES (?1, ?2, datetime('now'))",
                    params![FINGERPRINT_KEY, fp],
                )?;

                conn.execute_batch("COMMIT")
                    .context("Failed to commit reconciliation transaction")?;

                crate::logging::log_general("[RECONCILER] Schema reconciliation complete");
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e).context("Schema reconciliation failed")
            }
        }
    }

    fn execute_inner(self, db: &Database) -> Result<()> {
        let conn = db.conn();

        // 1. Create missing tables + their indices
        for t in &self.new_tables {
            crate::logging::log_general(format!("[RECONCILER] Creating table: {}", t.name));
            conn.execute_batch(&t.create_sql)
                .with_context(|| format!("Failed to create table '{}'", t.name))?;
            for idx in &t.index_sql {
                conn.execute_batch(idx)?;
            }
        }

        // 2. Add missing columns to core/decision tables
        for c in &self.new_columns {
            crate::logging::log_general(format!(
                "[RECONCILER] Adding column: {}.{}",
                c.table, c.column
            ));
            conn.execute(
                &format!(
                    "ALTER TABLE {} ADD COLUMN {} {}",
                    c.table, c.column, c.column_def
                ),
                [],
            )
            .with_context(|| format!("Failed to add column '{}.{}'", c.table, c.column))?;
        }

        // 3. Drop+recreate computed signal tables, seed dirty inodes
        for s in &self.recreated_signals {
            crate::logging::log_general(format!(
                "[RECONCILER] Recreating signal table: {}",
                s.table
            ));
            conn.execute_batch(&format!("DROP TABLE IF EXISTS {}", s.table))?;
            conn.execute_batch(&s.create_sql)
                .with_context(|| format!("Failed to recreate table '{}'", s.table))?;

            // Seed all corpus inodes as dirty so signals get recomputed
            seed_dirty_inodes_all(conn)?;
        }

        // 4. Create missing indices on existing tables
        for i in &self.new_indices {
            crate::logging::log_general("[RECONCILER] Creating index");
            conn.execute_batch(&i.create_sql)
                .context("Failed to create index")?;
        }

        // 5. Run data migrations
        for m in &self.data_migrations {
            crate::logging::log_general(format!(
                "[RECONCILER] Running data migration: {}",
                m.description
            ));
            (m.apply)(db)?;
            data_migrations::mark_migration_applied(db, m.id)?;
        }

        Ok(())
    }
}

/// Check if any data migrations are pending.
pub fn has_pending_data_migrations(db: &Database) -> bool {
    data_migrations::all_data_migrations()
        .iter()
        .any(|m| !data_migrations::is_migration_applied(db, m.id))
}

/// Check if the schema fingerprint matches (fast-path for startup).
pub fn fingerprint_matches(db: &Database) -> bool {
    let expected = table_schema::schema_fingerprint().to_string();
    let actual: Option<String> = db
        .conn()
        .query_row(
            "SELECT value FROM app_metadata WHERE key = ?1",
            [FINGERPRINT_KEY],
            |row| row.get(0),
        )
        .ok();
    actual.as_deref() == Some(expected.as_str())
}

// ============================================================================
// Introspection Helpers
// ============================================================================

/// Get all table names in the database.
fn get_table_names(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut names = HashSet::new();
    for name in rows {
        names.insert(name?);
    }
    Ok(names)
}

/// Get column info for a table via `pragma_table_info`.
fn get_column_info(conn: &Connection, table: &str) -> Result<Vec<ColumnInfo>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info('{}')", table))?;
    let rows = stmt.query_map([], |row| {
        Ok(ColumnInfo {
            name: row.get(1)?,
            col_type: row.get(2)?,
            notnull: row.get(3)?,
            dflt_value: row.get(4)?,
            _pk: row.get(5)?,
        })
    })?;
    let mut cols = Vec::new();
    for col in rows {
        cols.push(col?);
    }
    Ok(cols)
}

/// Build a column definition string from ColumnInfo (for ALTER TABLE ADD COLUMN).
fn build_column_def(col: &ColumnInfo) -> String {
    let mut def = col.col_type.clone();
    if col.notnull {
        def.push_str(" NOT NULL");
    }
    if let Some(ref dflt) = col.dflt_value {
        def.push_str(&format!(" DEFAULT {}", dflt));
    }
    def
}

/// Get existing index names for a table.
fn get_index_names(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = ?1 AND name NOT LIKE 'sqlite_%'",
    )?;
    let rows = stmt.query_map([table], |row| row.get::<_, String>(0))?;
    let mut names = HashSet::new();
    for name in rows {
        names.insert(name?);
    }
    Ok(names)
}

/// Check for missing indices and add them to the plan.
fn check_missing_indices(
    conn: &Connection,
    entry: &TableEntry,
    plan: &mut ReconciliationPlan,
) -> Result<()> {
    if entry.index_sql.is_empty() {
        return Ok(());
    }

    let existing = get_index_names(conn, entry.name)?;

    for idx_sql in entry.index_sql {
        // Extract index name from "CREATE INDEX IF NOT EXISTS idx_name ON ..."
        let idx_name = idx_sql
            .split("IF NOT EXISTS ")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .unwrap_or("");

        if !idx_name.is_empty() && !existing.contains(idx_name) {
            plan.new_indices.push(NewIndex {
                create_sql: idx_sql.to_string(),
            });
        }
    }

    Ok(())
}

/// Re-seed all corpus inodes as dirty for all computation types.
///
/// Used after recreating signal tables to trigger full recomputation.
fn seed_dirty_inodes_all(conn: &Connection) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Get all computation types that have dirty_inodes entries or are known
    // We seed for a broad set of computation types so everything recomputes
    let computation_types = [
        "file_in_corpus",
        "unindexed_file",
        "healthy_file",
        "corrupt_file",
        "shit_format",
        "compound_tag",
        "missing_tag",
        "path_tag_mismatch",
        "deploy_ready",
        "oob_tag_sync",
        "external_match",
        "sidecar_deploy",
    ];

    for comp_type in &computation_types {
        conn.execute(
            r#"
            INSERT OR IGNORE INTO dirty_inodes (inode, computation_type, dirtied_at)
            SELECT a.inode, ?1, ?2
            FROM audio_info a
            JOIN files f ON a.inode = f.inode
            WHERE f.zone = 'corpus'
            "#,
            params![comp_type, now],
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::table_schema::schema_inventory;
    use mm_utils::t;

    #[test]
    fn test_fresh_db_no_reconciliation_needed() {
        let conn = t!(Connection::open_in_memory());

        // Create all tables from inventory (simulating initialize_schema)
        for entry in schema_inventory() {
            t!(conn.execute_batch(entry.create_sql));
            for idx in entry.index_sql {
                t!(conn.execute_batch(idx));
            }
        }

        let plan = t!(ReconciliationPlan::compute(&conn));
        assert!(plan.is_empty(), "Fresh DB should need no reconciliation");
    }

    #[test]
    fn test_missing_table_detected() {
        let conn = t!(Connection::open_in_memory());

        // Create all tables EXCEPT one
        let inventory = schema_inventory();
        for (i, entry) in inventory.iter().enumerate() {
            if i == 0 {
                continue; // Skip first table
            }
            t!(conn.execute_batch(entry.create_sql));
            for idx in entry.index_sql {
                t!(conn.execute_batch(idx));
            }
        }

        let plan = t!(ReconciliationPlan::compute(&conn));
        assert!(!plan.is_empty(), "Should detect missing table");
        assert_eq!(plan.new_tables.len(), 1);
        assert_eq!(plan.new_tables[0].name, inventory[0].name);
    }

    #[test]
    fn test_missing_column_on_core_table() {
        let conn = t!(Connection::open_in_memory());

        // Create audio_info WITHOUT pic_count column
        t!(conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS audio_info (
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
                needs_tag_flush INTEGER NOT NULL DEFAULT 0,
                tags_version INTEGER NOT NULL DEFAULT 0
            )",
        ));

        // Create all other tables normally
        for entry in schema_inventory() {
            if entry.name == "audio_info" {
                continue;
            }
            t!(conn.execute_batch(entry.create_sql));
            for idx in entry.index_sql {
                t!(conn.execute_batch(idx));
            }
        }

        let plan = t!(ReconciliationPlan::compute(&conn));
        assert!(!plan.is_empty(), "Should detect missing column");
        assert!(
            plan.new_columns
                .iter()
                .any(|c| c.table == "audio_info" && c.column == "pic_count"),
            "Should detect missing pic_count column"
        );
    }

    #[test]
    fn test_column_diff_on_computed_table_triggers_recreate() {
        let conn = t!(Connection::open_in_memory());

        // Create signal_file_in_corpus WITHOUT the generation column
        t!(conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS signal_file_in_corpus (
                inode INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
        ));

        // Also need files/audio_info for the dirty inode seeding
        t!(conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                inode INTEGER NOT NULL, zone TEXT NOT NULL, path TEXT NOT NULL,
                is_dir INTEGER NOT NULL, mtime_secs INTEGER NOT NULL,
                mtime_nanos INTEGER NOT NULL, file_size INTEGER NOT NULL,
                scanned_at INTEGER NOT NULL, PRIMARY KEY (inode, zone, path)
            )",
        ));
        t!(conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS audio_info (
                inode INTEGER PRIMARY KEY, file_type TEXT NOT NULL
            )",
        ));
        t!(conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS dirty_inodes (
                inode INTEGER NOT NULL, computation_type TEXT NOT NULL,
                dirtied_at INTEGER NOT NULL, PRIMARY KEY (inode, computation_type)
            )",
        ));

        // Create all other tables normally
        for entry in schema_inventory() {
            if entry.name == "signal_file_in_corpus"
                || entry.name == "files"
                || entry.name == "audio_info"
                || entry.name == "dirty_inodes"
            {
                continue;
            }
            t!(conn.execute_batch(entry.create_sql));
        }

        let plan = t!(ReconciliationPlan::compute(&conn));
        assert!(
            plan.recreated_signals
                .iter()
                .any(|s| s.table == "signal_file_in_corpus"),
            "Should detect computed table needing recreation"
        );
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let fp1 = table_schema::schema_fingerprint();
        let fp2 = table_schema::schema_fingerprint();
        assert_eq!(fp1, fp2, "Fingerprint must be deterministic");
    }
}
