//! Scan state operations for incremental scanning.

use anyhow::{Context, Result};
use rusqlite::params;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::Database;
use crate::corpus::db::types::ScanStateEntry;

impl Database {
    // ========================================================================
    // Scan State Operations
    // ========================================================================

    pub fn clear_scan_state(&self, source: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM scan_state WHERE source = ?1", params![source])
            .context("Failed to clear scan state")?;
        Ok(())
    }

    /// Clean up scan_state entries where the file no longer exists on disk.
    /// This handles orphaned entries (in scan_state but not in tracks).
    /// Returns the number of entries deleted.
    pub fn cleanup_missing_scan_state_entries(&self, source: &str) -> Result<usize> {
        // Get all scan_state entries for this source
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM scan_state WHERE source = ?1")?;

        let paths: Vec<String> = stmt
            .query_map(params![source], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        // Find which paths no longer exist
        let missing_paths: Vec<String> = paths
            .into_iter()
            .filter(|p| !Path::new(p).exists())
            .collect();

        if missing_paths.is_empty() {
            return Ok(0);
        }

        // Delete orphaned entries
        let mut deleted = 0;
        for path in &missing_paths {
            deleted += self
                .conn
                .execute("DELETE FROM scan_state WHERE path = ?1", params![path])?;
        }

        Ok(deleted)
    }

    pub fn get_scan_state_batch(
        &self,
        source: &str,
        inodes: &[i64],
    ) -> Result<HashMap<i64, ScanStateEntry>> {
        if inodes.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = (0..inodes.len()).map(|_| "?").collect::<Vec<_>>().join(",");

        let query = format!(
            "SELECT source, inode, path, mtime_secs, mtime_nanos, file_size
             FROM scan_state
             WHERE source = ? AND inode IN ({})",
            placeholders
        );

        let mut stmt = self.conn.prepare(&query)?;

        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&source];
        for inode in inodes {
            params.push(inode);
        }

        let entries = stmt.query_map(&params[..], |row| {
            Ok(ScanStateEntry {
                source: row.get(0)?,
                inode: row.get(1)?,
                path: row.get(2)?,
                mtime_secs: row.get(3)?,
                mtime_nanos: row.get(4)?,
                file_size: row.get(5)?,
            })
        })?;

        let mut map = HashMap::new();
        for entry in entries {
            let entry = entry?;
            map.insert(entry.inode, entry);
        }

        Ok(map)
    }

    pub fn upsert_scan_state(&self, entry: &ScanStateEntry) -> Result<()> {
        self.conn
            .execute(
                r#"
            INSERT INTO scan_state (source, inode, path, mtime_secs, mtime_nanos, file_size)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(source, inode) DO UPDATE SET
                path = excluded.path,
                mtime_secs = excluded.mtime_secs,
                mtime_nanos = excluded.mtime_nanos,
                file_size = excluded.file_size,
                scanned_at = CURRENT_TIMESTAMP
            "#,
                params![
                    &entry.source,
                    &entry.inode,
                    &entry.path,
                    &entry.mtime_secs,
                    &entry.mtime_nanos,
                    &entry.file_size,
                ],
            )
            .context("Failed to upsert scan state")?;
        Ok(())
    }

    pub fn cleanup_stale_scan_state(
        &self,
        source: &str,
        current_inodes: &HashSet<i64>,
    ) -> Result<usize> {
        if current_inodes.is_empty() {
            let deleted = self
                .conn
                .execute("DELETE FROM scan_state WHERE source = ?1", params![source])?;
            return Ok(deleted);
        }

        let mut stmt = self
            .conn
            .prepare("SELECT inode FROM scan_state WHERE source = ?1")?;
        let existing_inodes: HashSet<i64> = stmt
            .query_map(params![source], |row| row.get(0))?
            .collect::<Result<HashSet<_>, _>>()?;

        let to_delete: Vec<i64> = existing_inodes
            .difference(current_inodes)
            .copied()
            .collect();

        if to_delete.is_empty() {
            return Ok(0);
        }

        let placeholders = (0..to_delete.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");

        let query = format!(
            "DELETE FROM scan_state WHERE source = ? AND inode IN ({})",
            placeholders
        );

        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&source];
        for inode in &to_delete {
            params.push(inode);
        }

        let deleted = self.conn.execute(&query, &params[..])?;
        Ok(deleted)
    }

    /// Get all indexed inodes for a source (used by heartbeat check)
    pub fn get_all_scan_state_inodes(&self, source: &str) -> Result<HashSet<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT inode FROM scan_state WHERE source = ?1")?;
        let inodes: HashSet<i64> = stmt
            .query_map(params![source], |row| row.get(0))?
            .collect::<Result<HashSet<_>, _>>()?;

        Ok(inodes)
    }

    /// Get the path for a specific inode from scan_state (used for relocation detection)
    pub fn get_scan_state_path_for_inode(
        &self,
        source: &str,
        inode: i64,
    ) -> Result<Option<String>> {
        let path: Option<String> = self
            .conn
            .query_row(
                "SELECT path FROM scan_state WHERE source = ?1 AND inode = ?2",
                params![source, inode],
                |row| row.get(0),
            )
            .ok();
        Ok(path)
    }

    /// Get paths for specific inodes from scan_state (used for logging missing files)
    pub fn get_scan_state_paths_for_inodes(
        &self,
        source: &str,
        inodes: &HashSet<i64>,
    ) -> Result<Vec<String>> {
        if inodes.is_empty() {
            return Ok(Vec::new());
        }

        // Build IN clause for query
        let placeholders: Vec<String> = inodes.iter().map(|_| "?".to_string()).collect();
        let query = format!(
            "SELECT path FROM scan_state WHERE source = ?1 AND inode IN ({})",
            placeholders.join(",")
        );

        let mut stmt = self.conn.prepare(&query)?;

        // Build params: source first, then all inodes
        let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        param_values.push(Box::new(source.to_string()));
        for inode in inodes {
            param_values.push(Box::new(*inode));
        }
        let params: Vec<&dyn rusqlite::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();

        let paths: Vec<String> = stmt
            .query_map(&params[..], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(paths)
    }
}
