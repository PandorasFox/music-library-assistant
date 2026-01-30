//! Library scan state queries.
//!
//! This module handles persistence of library file scan results between
//! computation phases:
//! - **Awakening**: ScanLibraryDirectory writes discovered files here
//! - **Awake**: DeriveDeployHealthSignals reads and processes this data
//!
//! Write operations require a witness for authorized execution.

use anyhow::{Context, Result};
use rusqlite::params;
use std::path::PathBuf;

use super::Database;
use crate::db_thread::SignalWitness;

/// A library file discovered during scanning.
#[derive(Debug, Clone)]
pub struct LibraryScanEntry {
    pub file_path: PathBuf,
    pub inode: i64,
}

impl Database {
    /// Clear all scan state for a library before re-scanning.
    ///
    /// Called at the start of WalkLibrary to ensure fresh scan results.
    pub fn clear_library_scan_state(&self, library_name: &str, _witness: &impl SignalWitness) -> Result<usize> {
        let count = self
            .conn
            .execute(
                "DELETE FROM library_scan_state WHERE library_name = ?1",
                params![library_name],
            )
            .context("Failed to clear library scan state")?;
        Ok(count)
    }

    /// Record a file discovered during library scanning.
    ///
    /// Called by ScanLibraryDirectory (Awakening phase) for each audio file found.
    pub fn record_library_file(
        &self,
        library_name: &str,
        library_root: &std::path::Path,
        file_path: &std::path::Path,
        inode: i64,
        scanned_at: i64,
        _witness: &impl SignalWitness,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO library_scan_state
                 (library_name, library_root, file_path, inode, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    library_name,
                    library_root.to_string_lossy().as_ref(),
                    file_path.to_string_lossy().as_ref(),
                    inode,
                    scanned_at
                ],
            )
            .context("Failed to record library file")?;
        Ok(())
    }

    /// Get all files for a library (for DeriveDeployHealthSignals).
    ///
    /// Returns all files discovered during the most recent scan of the library.
    pub fn get_library_scan_files(&self, library_name: &str) -> Result<Vec<LibraryScanEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, inode
             FROM library_scan_state
             WHERE library_name = ?1
             ORDER BY file_path",
        )?;

        let entries = stmt
            .query_map(params![library_name], |row| {
                Ok(LibraryScanEntry {
                    file_path: PathBuf::from(row.get::<_, String>(0)?),
                    inode: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to query library scan files")?;

        Ok(entries)
    }

    /// Get all files across all libraries.
    ///
    /// Returns all files discovered during library scans, for corpus-side deploy status.
    pub fn get_library_scan_files_all(&self) -> Result<Vec<LibraryScanEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, inode FROM library_scan_state ORDER BY file_path",
        )?;

        let entries = stmt
            .query_map(params![], |row| {
                Ok(LibraryScanEntry {
                    file_path: PathBuf::from(row.get::<_, String>(0)?),
                    inode: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to query all library scan files")?;

        Ok(entries)
    }

    /// Get all inodes that are deployed in any library.
    ///
    /// Returns a HashSet for O(1) lookup when checking if a corpus file is deployed.
    pub fn get_all_library_scan_inodes(&self) -> Result<std::collections::HashSet<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT inode FROM library_scan_state")?;

        let inodes = stmt
            .query_map(params![], |row| row.get(0))?
            .collect::<Result<std::collections::HashSet<i64>, _>>()
            .context("Failed to query library scan inodes")?;

        Ok(inodes)
    }
}
