//! Database schema definition and version management.
//!
//! `initialize_schema()` creates the full current schema for new databases
//! by iterating the schema inventory. The inventory is the single source
//! of truth — see `table_schema.rs`.

use anyhow::{Context, Result};
use rusqlite::params;

use super::queries::Database;
use super::table_schema::{schema_fingerprint, schema_inventory, FINGERPRINT_KEY};

impl Database {
    /// Create the full current schema from scratch (new databases only).
    ///
    /// Iterates the schema inventory and creates all tables + indices.
    pub(in crate::db) fn initialize_schema(&self) -> Result<()> {
        for entry in schema_inventory() {
            self.conn()
                .execute_batch(entry.create_sql)
                .with_context(|| format!("Failed to create table '{}'", entry.name))?;
            for idx in entry.index_sql {
                self.conn()
                    .execute_batch(idx)
                    .with_context(|| format!("Failed to create index for '{}'", entry.name))?;
            }
        }

        // Store initial schema fingerprint
        let fp = schema_fingerprint().to_string();
        self.conn().execute(
            "INSERT OR REPLACE INTO app_metadata (key, value, updated_at) VALUES (?1, ?2, datetime('now'))",
            params![FINGERPRINT_KEY, fp],
        ).context("Failed to store initial schema fingerprint")?;

        // Store initial blob schema versions
        for (table_name, version) in crate::meta::signals::registry::signal_blob_versions() {
            let key = format!("blob_version:{}", table_name);
            self.conn().execute(
                "INSERT OR REPLACE INTO app_metadata (key, value, updated_at) VALUES (?1, ?2, datetime('now'))",
                params![key, version.to_string()],
            )?;
        }

        Ok(())
    }
}
